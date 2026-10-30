// Answers lookup queries against the analyzed program and writes the results as plain text.
// Args: <output file> <query>...
//   str:<text>      functions referencing a string that contains <text> (ASCII or UTF-16)
//   imp:<name>      functions calling an imported function
//   fn:<hex addr>   decompile the function containing this address
//   callers:<hex>   functions calling the function at this address
//   bytes:<hex addr>:<count>  raw bytes at an address
// @category CnC
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashSet;
import java.util.Set;

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;

public class Query extends GhidraScript {
    private PrintWriter out;
    private DecompInterface decompiler;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        out = new PrintWriter(args[0], StandardCharsets.UTF_8);
        decompiler = new DecompInterface();
        decompiler.openProgram(currentProgram);
        try {
            for (int i = 1; i < args.length; i++) {
                String q = args[i];
                out.println("==== " + q);
                if (q.startsWith("str:")) {
                    queryString(q.substring(4));
                } else if (q.startsWith("imp:")) {
                    queryImport(q.substring(4));
                } else if (q.startsWith("fn:")) {
                    decompile(getFunctionContaining(toAddr(Long.parseUnsignedLong(q.substring(3), 16))));
                } else if (q.startsWith("callers:")) {
                    queryCallers(toAddr(Long.parseUnsignedLong(q.substring(8), 16)));
                } else if (q.startsWith("mkfn:")) {
                    Address a = toAddr(Long.parseUnsignedLong(q.substring(5), 16));
                    if (getFunctionAt(a) == null) {
                        disassemble(a);
                        createFunction(a, null);
                    }
                    decompile(getFunctionAt(a));
                } else if (q.startsWith("disasm:")) {
                    String[] parts = q.substring(7).split(":");
                    disasm(toAddr(Long.parseUnsignedLong(parts[0], 16)), Integer.parseInt(parts[1]));
                } else if (q.startsWith("sym:")) {
                    String needle = q.substring(4).toLowerCase();
                    int shown = 0;
                    for (Symbol s : currentProgram.getSymbolTable().getAllSymbols(true)) {
                        if (s.getName(true).toLowerCase().contains(needle) && shown++ < 200) {
                            out.println(s.getName(true) + " @" + s.getAddress());
                        }
                    }
                } else if (q.startsWith("fns:")) {
                    String[] parts = q.substring(4).split(":");
                    listFunctions(toAddr(Long.parseUnsignedLong(parts[0], 16)), toAddr(Long.parseUnsignedLong(parts[1], 16)));
                } else if (q.startsWith("table:")) {
                    String[] parts = q.substring(6).split(":");
                    dumpTable(toAddr(Long.parseUnsignedLong(parts[0], 16)), Integer.parseInt(parts[1]),
                        Integer.parseInt(parts[2]));
                } else if (q.startsWith("find:")) {
                    findBytes(q.substring(5));
                } else if (q.startsWith("ptr:")) {
                    queryPointer(Long.parseUnsignedLong(q.substring(4), 16));
                } else if (q.startsWith("bytes:")) {
                    String[] parts = q.substring(6).split(":");
                    dumpBytes(toAddr(Long.parseUnsignedLong(parts[0], 16)), Integer.parseInt(parts[1]));
                } else {
                    out.println("unknown query");
                }
                out.println();
            }
        } finally {
            decompiler.dispose();
            out.close();
        }
    }

    private void queryString(String text) throws Exception {
        Set<Address> starts = new LinkedHashSet<>();
        findAll(text.getBytes(StandardCharsets.US_ASCII), starts);
        findAll(text.getBytes(StandardCharsets.UTF_16LE), starts);
        for (Address start : starts) {
            out.println("string @" + start + " \"" + stringAt(start) + "\"");
            for (Reference ref : getReferencesTo(start)) {
                Function f = getFunctionContaining(ref.getFromAddress());
                out.println("  ref from " + ref.getFromAddress() + " in " + describe(f));
            }
        }
    }

    private void findAll(byte[] needle, Set<Address> starts) {
        Memory mem = currentProgram.getMemory();
        Address cur = mem.getMinAddress();
        while (cur != null) {
            Address hit = mem.findBytes(cur, needle, null, true, monitor);
            if (hit == null) {
                break;
            }
            Data d = getDataContaining(hit);
            starts.add(d != null ? d.getMinAddress() : hit);
            cur = hit.add(1);
        }
    }

    private String stringAt(Address a) {
        Data d = getDataAt(a);
        if (d != null && d.hasStringValue()) {
            return String.valueOf(d.getValue()).replace("\n", "\\n");
        }
        return "(undefined data)";
    }

    private void queryImport(String name) throws Exception {
        Set<Function> callers = new LinkedHashSet<>();
        for (Symbol sym : currentProgram.getSymbolTable().getSymbols(name)) {
            if (!sym.isExternal()) {
                continue;
            }
            for (Reference slotRef : getReferencesTo(sym.getAddress())) {
                Address slot = slotRef.getFromAddress();
                if (getDataAt(slot) == null) {
                    continue;
                }
                out.println("import slot " + slot);
                for (Reference call : getReferencesTo(slot)) {
                    Function f = getFunctionContaining(call.getFromAddress());
                    out.println("  used at " + call.getFromAddress() + " in " + describe(f));
                    if (f != null) {
                        callers.add(f);
                    }
                }
            }
        }
        out.println("distinct callers: " + callers.size());
    }

    private void queryCallers(Address target) {
        for (Reference ref : getReferencesTo(target)) {
            Function f = getFunctionContaining(ref.getFromAddress());
            out.println("  " + ref.getReferenceType() + " from " + ref.getFromAddress() + " in " + describe(f));
        }
    }

    private void disasm(Address start, int count) throws Exception {
        disassemble(start);
        Instruction ins = getInstructionAt(start);
        for (int i = 0; i < count && ins != null; i++) {
            StringBuilder raw = new StringBuilder();
            for (byte b : ins.getBytes()) {
                raw.append(String.format("%02X ", b));
            }
            out.println(ins.getAddress() + "  " + String.format("%-24s", raw.toString().trim()) + "  " + ins);
            ins = ins.getNext();
        }
    }

    /** Lists functions in a range with size, callers, and strings and callees they reference. */
    private void listFunctions(Address start, Address end) throws Exception {
        for (Function f : currentProgram.getFunctionManager().getFunctions(start, true)) {
            if (f.getEntryPoint().compareTo(end) >= 0) {
                break;
            }
            StringBuilder sb = new StringBuilder();
            sb.append(describe(f)).append(" size ").append(f.getBody().getNumAddresses());
            sb.append(" callers ").append(f.getCallingFunctions(monitor).size());
            Set<String> notes = new LinkedHashSet<>();
            for (Function callee : f.getCalledFunctions(monitor)) {
                notes.add(callee.getName());
            }
            for (Address a : f.getBody().getAddresses(true)) {
                for (Reference r : getReferencesFrom(a)) {
                    Data d = getDataAt(r.getToAddress());
                    if (d != null && d.hasStringValue()) {
                        notes.add("\"" + d.getValue() + "\"");
                    }
                }
            }
            sb.append(" -> ").append(String.join(", ", notes));
            out.println(sb);
        }
    }

    /** Prints each entry's dwords, with the C string behind any dword that points at one. */
    private void dumpTable(Address start, int entries, int stride) throws Exception {
        for (int i = 0; i < entries; i++) {
            Address entry = start.add((long) i * stride);
            StringBuilder sb = new StringBuilder(entry.toString()).append(":");
            for (int off = 0; off < stride; off += 4) {
                long v = getInt(entry.add(off)) & 0xffffffffL;
                sb.append(String.format(" %08X", v));
                Address target = toAddr(v);
                if (v > 0x400000 && currentProgram.getMemory().contains(target)) {
                    String s = readCString(target);
                    if (s != null) {
                        sb.append(" \"").append(s).append("\"");
                    }
                }
            }
            out.println(sb);
        }
    }

    private String readCString(Address a) {
        StringBuilder sb = new StringBuilder();
        try {
            for (int i = 0; i < 64; i++) {
                byte b = getByte(a.add(i));
                if (b == 0) {
                    return sb.length() >= 2 ? sb.toString() : null;
                }
                if (b < 0x20 || b > 0x7e) {
                    return null;
                }
                sb.append((char) b);
            }
        } catch (Exception e) {
            return null;
        }
        return null;
    }

    /** Hex bytes, with ?? as a wildcard byte. Hits inside functions are listed with the function. */
    private void findBytes(String hex) throws Exception {
        String clean = hex.replace(" ", "");
        byte[] needle = new byte[clean.length() / 2];
        byte[] mask = new byte[needle.length];
        for (int i = 0; i < needle.length; i++) {
            String b = clean.substring(i * 2, i * 2 + 2);
            if (b.equals("??")) {
                needle[i] = 0;
                mask[i] = 0;
            } else {
                needle[i] = (byte) Integer.parseInt(b, 16);
                mask[i] = (byte) 0xff;
            }
        }
        Memory mem = currentProgram.getMemory();
        Address cur = mem.getMinAddress();
        while (cur != null) {
            Address hit = mem.findBytes(cur, needle, mask, true, monitor);
            if (hit == null) {
                break;
            }
            out.println("found at " + hit + " in " + describe(getFunctionContaining(hit)));
            for (Reference ref : getReferencesTo(hit)) {
                out.println("  ref from " + ref.getFromAddress() + " in " + describe(getFunctionContaining(ref.getFromAddress())));
            }
            cur = hit.add(1);
        }
    }

    private void queryPointer(long value) throws Exception {
        byte[] needle = new byte[] {
            (byte) value, (byte) (value >> 8), (byte) (value >> 16), (byte) (value >> 24)
        };
        Memory mem = currentProgram.getMemory();
        Address cur = mem.getMinAddress();
        while (cur != null) {
            Address hit = mem.findBytes(cur, needle, null, true, monitor);
            if (hit == null) {
                break;
            }
            out.print("pointer at " + hit + " in " + describe(getFunctionContaining(hit)) + ", next 16 bytes: ");
            dumpBytes(hit, 16);
            cur = hit.add(1);
        }
    }

    private void dumpBytes(Address a, int count) throws Exception {
        byte[] buf = getBytes(a, count);
        StringBuilder sb = new StringBuilder();
        for (byte b : buf) {
            sb.append(String.format("%02X ", b));
        }
        out.println(sb.toString().trim());
    }

    private void decompile(Function f) {
        if (f == null) {
            out.println("no function there");
            return;
        }
        out.println("function " + describe(f));
        DecompileResults res = decompiler.decompileFunction(f, 120, monitor);
        if (res.decompileCompleted()) {
            out.println(res.getDecompiledFunction().getC());
        } else {
            out.println("decompile failed: " + res.getErrorMessage());
        }
    }

    private String describe(Function f) {
        return f == null ? "(no function)" : f.getName() + " @" + f.getEntryPoint();
    }
}
