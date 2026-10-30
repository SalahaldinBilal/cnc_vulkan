// Finds code from one build in another, for porting addresses between Zero Hour and Generals.
// Args: sig <output file> <list file>   (on the source program) list lines: "<hex addr> <length>"
//       find <output file> <sig file>   (on the target program)
// sig writes, for each address, masked byte patterns of the instructions around it at a few
// widths: every operand that refers to an address (calls, jumps, globals, vtables) is masked,
// since those move between builds. find searches each pattern, widest unique match wins, and
// prints the target address with the bytes found there.
// @category CnC
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.List;

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.lang.InstructionPrototype;
import ghidra.program.model.lang.Mask;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.scalar.Scalar;

public class Port extends GhidraScript {
    private static final int[] RADII = {32, 64, 16, 128};
    /** Loose patterns, which also mask every displacement and immediate (struct offsets move too). */
    private static final int[] LOOSE_RADII = {48, 96, 160};
    private static final long IMAGE_LO = 0x400000L, IMAGE_HI = 0xC00000L;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        try (PrintWriter out = new PrintWriter(args[1], StandardCharsets.UTF_8)) {
            List<String> lines = Files.readAllLines(Paths.get(args[2]));
            if (args[0].equals("sig")) {
                for (String line : lines) {
                    signatures(out, line.trim());
                }
            } else {
                for (String line : lines) {
                    find(out, line.trim());
                }
            }
        }
    }

    private void signatures(PrintWriter out, String line) throws Exception {
        if (line.isEmpty()) {
            return;
        }
        String[] p = line.split("\\s+");
        Address at = toAddr(Long.parseUnsignedLong(p[0], 16));
        int len = Integer.parseInt(p[1]);
        var block = currentProgram.getMemory().getBlock(at);
        if (block == null || !block.isExecute()) {
            out.println(p[0] + " " + len + " DATA " + hex(getBytes(at, len)));
            return;
        }
        Instruction ins = getInstructionContaining(at);
        if (ins == null) {
            disassemble(at);
            ins = getInstructionContaining(at);
        }
        if (ins == null) {
            out.println(p[0] + " " + len + " NOCODE " + hex(getBytes(at, len)));
            return;
        }
        StringBuilder sb = new StringBuilder(p[0] + " " + len + " CODE");
        for (int k = 0; k < RADII.length + LOOSE_RADII.length; k++) {
            boolean loose = k >= RADII.length;
            int radius = loose ? LOOSE_RADII[k - RADII.length] : RADII[k];
            Instruction first = ins;
            while (first.getPrevious() != null && first.getPrevious().getAddress().compareTo(at.subtract(radius)) >= 0
                && first.getPrevious().getMaxAddress().next().equals(first.getAddress())) {
                first = first.getPrevious();
            }
            ByteArrayBuilder bytes = new ByteArrayBuilder();
            Instruction cur = first;
            Address end = at.add(Math.max(len, radius));
            while (cur != null && cur.getAddress().compareTo(end) < 0) {
                byte[] b = cur.getBytes();
                byte[] m = mask(cur, loose);
                bytes.add(b, m);
                Instruction next = cur.getNext();
                if (next == null || !cur.getMaxAddress().next().equals(next.getAddress())) {
                    break;
                }
                cur = next;
            }
            long offset = at.subtract(first.getAddress());
            sb.append(" ").append(loose ? "L" : "").append(radius).append(":").append(offset).append(":").append(hex(bytes.bytes()))
                .append(":").append(hex(bytes.masks()));
        }
        out.println(sb);
    }

    /** All ones, except operands that are addresses in the image or references, or with loose
     * every operand that holds a value. */
    private byte[] mask(Instruction ins, boolean loose) throws Exception {
        byte[] m = new byte[ins.getLength()];
        java.util.Arrays.fill(m, (byte) 0xff);
        InstructionPrototype proto = ins.getPrototype();
        for (int op = 0; op < ins.getNumOperands(); op++) {
            boolean moves = ins.getOperandReferences(op).length > 0;
            for (Object o : ins.getOpObjects(op)) {
                if (o instanceof Scalar s) {
                    long v = s.getUnsignedValue();
                    moves |= loose || v >= IMAGE_LO && v < IMAGE_HI;
                }
                if (o instanceof Address a) {
                    long v = a.getOffset();
                    moves |= v >= IMAGE_LO && v < IMAGE_HI;
                }
            }
            if (!moves) {
                continue;
            }
            Mask om = proto.getOperandValueMask(op);
            if (om == null) {
                continue;
            }
            byte[] bits = om.getBytes();
            for (int i = 0; i < m.length && i < bits.length; i++) {
                m[i] &= (byte) ~bits[i];
            }
        }
        // Relative branches: the displacement moves even without a recorded reference.
        if (ins.getFlowType().isJump() || ins.getFlowType().isCall()) {
            int opcode = ins.getByte(0) & 0xff;
            if (opcode == 0xE8 || opcode == 0xE9) {
                for (int i = 1; i < m.length; i++) m[i] = 0;
            } else if (opcode == 0x0F && m.length == 6) {
                for (int i = 2; i < 6; i++) m[i] = 0;
            }
        }
        return m;
    }

    private void find(PrintWriter out, String line) throws Exception {
        if (line.isEmpty()) {
            return;
        }
        String[] p = line.split("\\s+");
        String source = p[0];
        int len = Integer.parseInt(p[1]);
        if (!p[2].equals("CODE")) {
            out.println(source + " " + p[2] + " " + p[3]);
            return;
        }
        String result = null;
        for (int i = 3; i < p.length; i++) {
            String[] s = p[i].split(":");
            String radius = s[0];
            long offset = Long.parseLong(s[1]);
            byte[] bytes = unhex(s[2]);
            byte[] masks = unhex(s[3]);
            List<Address> hits = search(bytes, masks, 3);
            if (hits.size() == 1) {
                Address target = hits.get(0).add(offset);
                result = source + " -> " + target + " r" + radius + " " + hex(getBytes(target, len));
                break;
            }
            String miss = hits.isEmpty() ? "none" : hits.size() + "+ matches";
            result = (result == null ? source + " -> " : result + ",") + miss + " r" + radius;
        }
        out.println(result);
    }

    private List<Address> search(byte[] bytes, byte[] masks, int limit) {
        List<Address> hits = new ArrayList<>();
        Memory mem = currentProgram.getMemory();
        Address cur = mem.getMinAddress();
        while (cur != null && hits.size() < limit) {
            Address hit = mem.findBytes(cur, bytes, masks, true, monitor);
            if (hit == null) {
                break;
            }
            hits.add(hit);
            cur = hit.add(1);
        }
        return hits;
    }

    private static String hex(byte[] b) {
        StringBuilder sb = new StringBuilder();
        for (byte x : b) {
            sb.append(String.format("%02X", x));
        }
        return sb.toString();
    }

    private static byte[] unhex(String s) {
        byte[] b = new byte[s.length() / 2];
        for (int i = 0; i < b.length; i++) {
            b[i] = (byte) Integer.parseInt(s.substring(i * 2, i * 2 + 2), 16);
        }
        return b;
    }

    private static class ByteArrayBuilder {
        private final java.io.ByteArrayOutputStream b = new java.io.ByteArrayOutputStream();
        private final java.io.ByteArrayOutputStream m = new java.io.ByteArrayOutputStream();

        void add(byte[] bytes, byte[] masks) {
            b.writeBytes(bytes);
            m.writeBytes(masks);
        }

        byte[] bytes() {
            return b.toByteArray();
        }

        byte[] masks() {
            return m.toByteArray();
        }
    }
}
