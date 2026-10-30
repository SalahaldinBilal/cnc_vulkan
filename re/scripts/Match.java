// Matches functions between Zero Hour and Generals, then maps listed Zero Hour addresses to Generals.
// Run on Generals, with Zero Hour at /zh/game.dat in the same project.
// Args: <output dir> [seed:<Port find output>] <address list>...   list lines: "<hex addr> [length] [label]"
// Appends progress.log after every step, rewrites functions.txt after every step, and writes
// <list name>.mapped.txt for each list at the end.
// @category CnC
import java.io.FileWriter;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.lang.InstructionPrototype;
import ghidra.program.model.lang.Mask;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.Listing;
import ghidra.program.model.listing.Program;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.Reference;

public class Match extends GhidraScript {
    private static final long FNV = 0xcbf29ce484222325L, PRIME = 0x100000001b3L;

    private Program zh, gen;
    private Path dir;
    private PrintWriter log;
    private long start;
    private final Map<Long, Fn> zhFns = new HashMap<>(), genFns = new HashMap<>();
    private final Map<Long, Long> z2g = new HashMap<>(), g2z = new HashMap<>();
    private final Map<Long, String> how = new HashMap<>();
    private final Map<String, int[]> alignments = new HashMap<>();
    private final Map<String, List<Instruction>> bodies = new HashMap<>();

    private static class Fn {
        long entry;
        Function f;
        long exact = FNV, mnem = FNV, strings;
        int count;
        Set<String> strs = new HashSet<>();
        List<Long> callees = new ArrayList<>();
        Set<Long> callers = new HashSet<>();
    }

    private record Mapped(long gen, String how) {
        boolean ok() {
            return gen >= 0;
        }
    }

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        dir = Paths.get(args[0]);
        Files.createDirectories(dir);
        start = System.currentTimeMillis();
        log = new PrintWriter(new FileWriter(dir.resolve("progress.log").toFile(), StandardCharsets.UTF_8, true), true);
        gen = currentProgram;
        zh = (Program) state.getProject().getProjectData().getFile("/zh/game.dat").getDomainObject(this, false, false, monitor);
        int zt = zh.startTransaction("orphans"), gt = gen.startTransaction("orphans");
        try {
            log("start");
            orphans(zh, "zh");
            orphans(gen, "generals");
            collect(zh, zhFns, "zh");
            collect(gen, genFns, "generals");
            for (int i = 1; i < args.length; i++) {
                if (args[i].startsWith("seed:")) {
                    seed(args[i].substring(5));
                }
            }
            uniqueBy("exact", f -> f.exact, 4);
            uniqueBy("mnemonics", f -> f.mnem, 10);
            uniqueBy("string-set", f -> f.strings, 1);
            uniqueStrings();
            writeMatches();
            for (int round = 1;; round++) {
                int added = propagate() + gaps() + uniqueBy("exact", f -> f.exact, 4) + uniqueBy("mnemonics", f -> f.mnem, 10);
                log("round " + round + ": +" + added);
                writeMatches();
                if (added == 0) {
                    break;
                }
            }
            for (int i = 1; i < args.length; i++) {
                if (!args[i].startsWith("seed:")) {
                    mapList(args[i]);
                }
            }
            log("done");
        } catch (Exception e) {
            log("failed: " + e);
            throw e;
        } finally {
            gen.endTransaction(gt, false);
            zh.endTransaction(zt, false);
            zh.release(this);
            log.close();
        }
    }

    /** Code that only tables reach (vtable methods, command line handlers) often has no function.
     * Makes one at every such spot, in memory only (the transaction is never committed). */
    private void orphans(Program p, String name) throws Exception {
        FunctionManager fm = p.getFunctionManager();
        int made = 0;
        for (Instruction ins : p.getListing().getInstructions(true)) {
            monitor.checkCancelled();
            if (fm.getFunctionContaining(ins.getAddress()) == null && new CreateFunctionCmd(ins.getAddress()).applyTo(p, monitor)) {
                made++;
            }
        }
        log(name + ": made " + made + " functions from code outside any");
    }

    private void log(String msg) {
        log.println(String.format("%7.1fs  matched %6d  %s", (System.currentTimeMillis() - start) / 1000.0, z2g.size(), msg));
    }

    private void collect(Program p, Map<Long, Fn> fns, String name) throws Exception {
        FunctionManager fm = p.getFunctionManager();
        Listing listing = p.getListing();
        long lo = p.getMinAddress().getOffset(), hi = p.getMaxAddress().getOffset();
        int n = 0;
        for (Function f : fm.getFunctions(true)) {
            monitor.checkCancelled();
            if (f.isThunk() || f.isExternal()) {
                continue;
            }
            Fn fn = new Fn();
            fn.entry = f.getEntryPoint().getOffset();
            fn.f = f;
            for (Instruction ins : listing.getInstructions(f.getBody(), true)) {
                fn.count++;
                byte[] b = ins.getBytes(), m = mask(ins, false, lo, hi);
                for (int i = 0; i < b.length; i++) {
                    fn.exact = (fn.exact ^ (b[i] & m[i] & 0xff)) * PRIME;
                }
                fn.exact = (fn.exact ^ 0x100) * PRIME;
                fn.mnem = (fn.mnem ^ ins.getMnemonicString().hashCode()) * PRIME;
                if (ins.getFlowType().isCall()) {
                    for (Address t : ins.getFlows()) {
                        Function c = fm.getFunctionAt(t);
                        if (c != null && c.isThunk()) {
                            c = c.getThunkedFunction(true);
                        }
                        if (c != null && !c.isExternal() && !fn.callees.contains(c.getEntryPoint().getOffset())) {
                            fn.callees.add(c.getEntryPoint().getOffset());
                        }
                    }
                }
                for (Reference r : ins.getReferencesFrom()) {
                    Data d = listing.getDataAt(r.getToAddress());
                    if (d != null && d.hasStringValue()) {
                        fn.strs.add(String.valueOf(d.getValue()));
                    }
                }
            }
            if (!fn.strs.isEmpty()) {
                List<String> sorted = new ArrayList<>(fn.strs);
                sorted.sort(null);
                fn.strings = hash(String.join("\u0000", sorted));
            }
            fns.put(fn.entry, fn);
            if (++n % 2000 == 0) {
                log(name + ": read " + n + " functions");
            }
        }
        for (Fn fn : fns.values()) {
            for (long c : fn.callees) {
                Fn cf = fns.get(c);
                if (cf != null) {
                    cf.callers.add(fn.entry);
                }
            }
        }
        log(name + ": " + fns.size() + " functions");
    }

    private static long hash(String s) {
        long h = FNV;
        for (int i = 0; i < s.length(); i++) {
            h = (h ^ s.charAt(i)) * PRIME;
        }
        return h;
    }

    /** All ones, except operands that are addresses in the image or references, or with loose
     * every operand that holds a value. */
    private static byte[] mask(Instruction ins, boolean loose, long lo, long hi) {
        byte[] m = new byte[ins.getLength()];
        Arrays.fill(m, (byte) 0xff);
        InstructionPrototype proto = ins.getPrototype();
        for (int op = 0; op < ins.getNumOperands(); op++) {
            boolean moves = ins.getOperandReferences(op).length > 0;
            for (Object o : ins.getOpObjects(op)) {
                if (o instanceof Scalar s) {
                    long v = s.getUnsignedValue();
                    moves |= loose || v >= lo && v <= hi;
                }
                if (o instanceof Address a) {
                    long v = a.getOffset();
                    moves |= v >= lo && v <= hi;
                }
            }
            Mask om = moves ? proto.getOperandValueMask(op) : null;
            if (om == null) {
                continue;
            }
            byte[] bits = om.getBytes();
            for (int i = 0; i < m.length && i < bits.length; i++) {
                m[i] &= (byte) ~bits[i];
            }
        }
        if (ins.getFlowType().isJump() || ins.getFlowType().isCall()) {
            try {
                int opcode = ins.getByte(0) & 0xff;
                if (opcode == 0xE8 || opcode == 0xE9) {
                    Arrays.fill(m, 1, m.length, (byte) 0);
                } else if (opcode == 0x0F && m.length == 6) {
                    Arrays.fill(m, 2, 6, (byte) 0);
                }
            } catch (Exception e) {
                // keep the mask
            }
        }
        return m;
    }

    private boolean add(long z, long g, String method) {
        if (z2g.containsKey(z) || g2z.containsKey(g)) {
            return false;
        }
        z2g.put(z, g);
        g2z.put(g, z);
        how.put(z, method);
        return true;
    }

    private void seed(String file) throws Exception {
        Pattern line = Pattern.compile("^([0-9a-fA-F]{8}) -> ([0-9a-fA-F]{8}) r\\d");
        int added = 0;
        for (String s : Files.readAllLines(Paths.get(file))) {
            Matcher m = line.matcher(s.trim());
            if (!m.find()) {
                continue;
            }
            Function zf = zh.getFunctionManager().getFunctionContaining(addr(zh, Long.parseLong(m.group(1), 16)));
            Function gf = gen.getFunctionManager().getFunctionContaining(addr(gen, Long.parseLong(m.group(2), 16)));
            if (zf != null && gf != null && zhFns.containsKey(zf.getEntryPoint().getOffset())
                && genFns.containsKey(gf.getEntryPoint().getOffset())
                && add(zf.getEntryPoint().getOffset(), gf.getEntryPoint().getOffset(), "seed")) {
                added++;
            }
        }
        log("seed: +" + added);
    }

    private int uniqueBy(String method, java.util.function.ToLongFunction<Fn> key, int minCount) {
        Map<Long, List<Fn>> a = group(zhFns, z2g, key, minCount), b = group(genFns, g2z, key, minCount);
        int added = 0;
        for (var e : a.entrySet()) {
            List<Fn> y = b.get(e.getKey());
            if (e.getValue().size() == 1 && y != null && y.size() == 1 && add(e.getValue().get(0).entry, y.get(0).entry, method)) {
                added++;
            }
        }
        log(method + ": +" + added);
        return added;
    }

    private static Map<Long, List<Fn>> group(Map<Long, Fn> fns, Map<Long, Long> matched, java.util.function.ToLongFunction<Fn> key, int minCount) {
        Map<Long, List<Fn>> out = new HashMap<>();
        for (Fn f : fns.values()) {
            long k = key.applyAsLong(f);
            if (f.count >= minCount && k != 0 && !matched.containsKey(f.entry)) {
                out.computeIfAbsent(k, x -> new ArrayList<>()).add(f);
            }
        }
        return out;
    }

    /** A string referenced by exactly one function in each build pairs those functions, unless the
     * function's strings disagree on its partner. */
    private void uniqueStrings() {
        Map<String, List<Long>> zs = byString(zhFns), gs = byString(genFns);
        Map<Long, Long> vote = new HashMap<>();
        Set<Long> conflict = new HashSet<>();
        for (var e : zs.entrySet()) {
            List<Long> g = gs.get(e.getKey());
            if (e.getValue().size() != 1 || g == null || g.size() != 1) {
                continue;
            }
            long z = e.getValue().get(0);
            Long prev = vote.put(z, g.get(0));
            if (prev != null && prev.longValue() != g.get(0)) {
                conflict.add(z);
            }
        }
        Map<Long, Integer> targets = new HashMap<>();
        vote.forEach((z, g) -> targets.merge(g, 1, Integer::sum));
        int added = 0;
        for (var e : vote.entrySet()) {
            if (!conflict.contains(e.getKey()) && targets.get(e.getValue()) == 1 && add(e.getKey(), e.getValue(), "string")) {
                added++;
            }
        }
        log("unique strings: +" + added);
    }

    private static Map<String, List<Long>> byString(Map<Long, Fn> fns) {
        Map<String, List<Long>> out = new HashMap<>();
        for (Fn f : fns.values()) {
            for (String s : f.strs) {
                out.computeIfAbsent(s, x -> new ArrayList<>()).add(f.entry);
            }
        }
        return out;
    }

    private boolean similar(long z, long g) {
        Fn a = zhFns.get(z), b = genFns.get(g);
        if (a == null || b == null) {
            return false;
        }
        if (a.mnem == b.mnem) {
            return true;
        }
        int lo = Math.min(a.count, b.count), hi = Math.max(a.count, b.count);
        return hi >= 8 && lo * 10 >= hi * 7;
    }

    private int propagate() {
        int added = 0;
        for (var e : new ArrayList<>(z2g.entrySet())) {
            Fn a = zhFns.get(e.getKey()), b = genFns.get(e.getValue());
            if (a == null || b == null) {
                continue;
            }
            added += pairInOrder(a.callees, b.callees);
            added += pairSets(new HashSet<>(a.callees), new HashSet<>(b.callees), "callee");
            added += pairSets(a.callers, b.callers, "caller");
        }
        log("calls: +" + added);
        return added;
    }

    private int pairInOrder(List<Long> x, List<Long> y) {
        if (x.size() != y.size() || x.isEmpty()) {
            return 0;
        }
        for (int i = 0; i < x.size(); i++) {
            Long g = z2g.get(x.get(i)), z = g2z.get(y.get(i));
            if (g != null && g.longValue() != y.get(i) || z != null && z.longValue() != x.get(i)) {
                return 0;
            }
        }
        int added = 0;
        for (int i = 0; i < x.size(); i++) {
            if (similar(x.get(i), y.get(i)) && add(x.get(i), y.get(i), "callee-order")) {
                added++;
            }
        }
        return added;
    }

    private int pairSets(Set<Long> x, Set<Long> y, String method) {
        List<Long> zx = new ArrayList<>(), gy = new ArrayList<>();
        for (long z : x) {
            if (!z2g.containsKey(z) && zhFns.containsKey(z)) {
                zx.add(z);
            }
        }
        for (long g : y) {
            if (!g2z.containsKey(g) && genFns.containsKey(g)) {
                gy.add(g);
            }
        }
        if (zx.isEmpty() || gy.isEmpty()) {
            return 0;
        }
        if (zx.size() == 1 && gy.size() == 1) {
            return similar(zx.get(0), gy.get(0)) && add(zx.get(0), gy.get(0), method + "-only") ? 1 : 0;
        }
        int added = 0;
        for (boolean exact : new boolean[] {true, false}) {
            Map<Long, List<Long>> a = new HashMap<>(), b = new HashMap<>();
            for (long z : zx) {
                Fn f = zhFns.get(z);
                a.computeIfAbsent(exact ? f.exact : f.mnem, k -> new ArrayList<>()).add(z);
            }
            for (long g : gy) {
                Fn f = genFns.get(g);
                b.computeIfAbsent(exact ? f.exact : f.mnem, k -> new ArrayList<>()).add(g);
            }
            for (var e : a.entrySet()) {
                List<Long> other = b.get(e.getKey());
                if (e.getValue().size() == 1 && other != null && other.size() == 1
                    && add(e.getValue().get(0), other.get(0), method + (exact ? "-exact" : "-mnemonics"))) {
                    added++;
                }
            }
        }
        return added;
    }

    /** Functions sit in source file order in both builds, so the unmatched functions between two
     * neighboring matches usually pair up in order. */
    private int gaps() {
        List<Long> zs = new ArrayList<>(zhFns.keySet()), gs = new ArrayList<>(genFns.keySet());
        zs.sort(null);
        gs.sort(null);
        Map<Long, Integer> gIndex = new HashMap<>();
        for (int i = 0; i < gs.size(); i++) {
            gIndex.put(gs.get(i), i);
        }
        List<int[]> pairs = new ArrayList<>();
        for (int i = 0; i < zs.size(); i++) {
            Long g = z2g.get(zs.get(i));
            if (g != null && gIndex.containsKey(g)) {
                pairs.add(new int[] {i, gIndex.get(g)});
            }
        }
        int added = 0;
        for (int k = 0; k + 1 < pairs.size(); k++) {
            int[] p = pairs.get(k), q = pairs.get(k + 1);
            int dz = q[0] - p[0] - 1, dg = q[1] - p[1] - 1;
            if (dz <= 0 || dg <= 0 || dz > 200 || dg > 200) {
                continue;
            }
            if (dz == dg) {
                boolean all = true;
                for (int i = 1; i <= dz && all; i++) {
                    all = !g2z.containsKey(gs.get(p[1] + i)) && similar(zs.get(p[0] + i), gs.get(p[1] + i));
                }
                if (all) {
                    for (int i = 1; i <= dz; i++) {
                        if (add(zs.get(p[0] + i), gs.get(p[1] + i), "gap")) {
                            added++;
                        }
                    }
                    continue;
                }
            }
            for (int i = 1; i <= Math.min(dz, dg); i++) {
                long z = zs.get(p[0] + i), g = gs.get(p[1] + i);
                if (g2z.containsKey(g) || zhFns.get(z).mnem != genFns.get(g).mnem || !add(z, g, "gap-edge")) {
                    break;
                }
                added++;
            }
            for (int i = 1; i <= Math.min(dz, dg); i++) {
                long z = zs.get(q[0] - i), g = gs.get(q[1] - i);
                if (z2g.containsKey(z) || g2z.containsKey(g) || zhFns.get(z).mnem != genFns.get(g).mnem || !add(z, g, "gap-edge")) {
                    break;
                }
                added++;
            }
        }
        log("gaps: +" + added);
        return added;
    }

    private void writeMatches() throws Exception {
        Path tmp = dir.resolve("functions.txt.tmp");
        try (PrintWriter w = new PrintWriter(tmp.toFile(), StandardCharsets.UTF_8)) {
            for (var e : new TreeMap<>(z2g).entrySet()) {
                Fn a = zhFns.get(e.getKey()), b = genFns.get(e.getValue());
                w.println(String.format("%08x %08x %s %d %d", e.getKey(), e.getValue(), how.get(e.getKey()), a.count, b.count));
            }
        }
        Files.move(tmp, dir.resolve("functions.txt"), StandardCopyOption.REPLACE_EXISTING);
    }

    private static Address addr(Program p, long offset) {
        return p.getAddressFactory().getDefaultAddressSpace().getAddress(offset);
    }

    private void mapList(String file) throws Exception {
        Path in = Paths.get(file);
        String name = in.getFileName().toString().replaceFirst("\\.txt$", "");
        int ok = 0, total = 0;
        try (PrintWriter w = new PrintWriter(dir.resolve(name + ".mapped.txt").toFile(), StandardCharsets.UTF_8)) {
            for (String line : Files.readAllLines(in)) {
                line = line.trim();
                if (line.isEmpty()) {
                    continue;
                }
                String[] p = line.split("\\s+");
                if (!p[0].matches("[0-9a-fA-F]+")) {
                    w.println(line);
                    continue;
                }
                long z = Long.parseLong(p[0], 16);
                int len = p.length > 1 && p[1].matches("\\d+") ? Integer.parseInt(p[1]) : 4;
                String label = line.substring(p[0].length()).trim();
                Mapped m;
                try {
                    m = map(z);
                } catch (Exception e) {
                    m = new Mapped(-1, "error " + e);
                }
                total++;
                if (m.ok()) {
                    ok++;
                    w.println(String.format("%08x -> %08x  %s  [%s]  zh %s  generals %s", z, m.gen(), m.how(), label,
                        hex(zh, z, len), hex(gen, m.gen(), len)));
                } else {
                    w.println(String.format("%08x -> ?  %s  [%s]", z, m.how(), label));
                }
                w.flush();
            }
        }
        log("mapped " + ok + " of " + total + " in " + in.getFileName());
    }

    private String hex(Program p, long at, int len) {
        try {
            byte[] b = new byte[len];
            p.getMemory().getBytes(addr(p, at), b);
            StringBuilder sb = new StringBuilder();
            for (byte x : b) {
                sb.append(String.format("%02X", x));
            }
            return sb.toString();
        } catch (Exception e) {
            return "-";
        }
    }

    private Mapped map(long z) throws Exception {
        Address a = addr(zh, z);
        FunctionManager fm = zh.getFunctionManager();
        if (fm.getFunctionAt(a) != null) {
            Long g = z2g.get(z);
            return g != null ? new Mapped(g, "function " + how.get(z)) : new Mapped(-1, "function not matched");
        }
        Function f = fm.getFunctionContaining(a);
        if (f != null) {
            return inFunction(f, a);
        }
        if (zh.getListing().getInstructionContaining(a) != null || zh.getMemory().getBlock(a).isExecute()) {
            return new Mapped(-1, "code outside any function");
        }
        Mapped r = byReferences(a);
        if (r.ok()) {
            return r;
        }
        Mapped p = byPointer(a);
        return p.ok() ? p : new Mapped(-1, r.how() + ", " + p.how());
    }

    private Mapped inFunction(Function f, Address a) throws Exception {
        long ze = f.getEntryPoint().getOffset();
        Long ge = z2g.get(ze);
        if (ge == null) {
            return new Mapped(-1, String.format("in unmatched function %08x", ze));
        }
        Mapped g = instruction(ze, ge, a);
        if (!g.ok()) {
            return new Mapped(-1, String.format("%s in %08x -> %08x", g.how(), ze, ge));
        }
        return new Mapped(g.gen(), (zhFns.get(ze).exact == genFns.get(ge).exact ? "same function " : g.how() + " in ")
            + String.format("%08x -> %08x (%s)", ze, ge, how.get(ze)));
    }

    /** The Generals address of the Zero Hour instruction at a, keeping its offset into the instruction.
     * An unaligned instruction maps by position when the aligned ones on both sides span the same count. */
    private Mapped instruction(long ze, long ge, Address a) {
        List<Instruction> zl = body(zh, ze), gl = body(gen, ge);
        int[] align = align(ze, ge, zl, gl);
        if (align == null) {
            return new Mapped(-1, "function too big to align");
        }
        for (int i = 0; i < zl.size(); i++) {
            Instruction ins = zl.get(i);
            if (!ins.contains(a)) {
                continue;
            }
            long into = a.subtract(ins.getAddress());
            if (align[i] >= 0) {
                return new Mapped(gl.get(align[i]).getAddress().getOffset() + into, "aligned");
            }
            int lo = i - 1, hi = i + 1;
            while (lo >= 0 && align[lo] < 0) {
                lo--;
            }
            while (hi < zl.size() && align[hi] < 0) {
                hi++;
            }
            int glo = lo >= 0 ? align[lo] : -1, ghi = hi < zl.size() ? align[hi] : gl.size();
            int zn = hi - lo - 1, gn = ghi - glo - 1;
            if (gn > 0 && (long) zn * gn <= 4_000_000L) {
                long[] x = new long[zn], y = new long[gn];
                for (int k = 0; k < zn; k++) {
                    x[k] = zl.get(lo + 1 + k).getMnemonicString().hashCode();
                }
                for (int k = 0; k < gn; k++) {
                    y[k] = gl.get(glo + 1 + k).getMnemonicString().hashCode();
                }
                int j = lcs(x, y)[i - lo - 1];
                if (j >= 0) {
                    return new Mapped(gl.get(glo + 1 + j).getAddress().getOffset() + into, "aligned by name");
                }
            }
            if (hi - lo == ghi - glo) {
                Instruction gi = gl.get(glo + (i - lo));
                if (gi.getMnemonicString().equals(ins.getMnemonicString()) && gi.getLength() == ins.getLength()) {
                    return new Mapped(gi.getAddress().getOffset() + into, "by position");
                }
            }
            return new Mapped(-1, "instruction not aligned");
        }
        return new Mapped(-1, "instruction not in body");
    }

    private List<Instruction> body(Program p, long entry) {
        return bodies.computeIfAbsent((p == zh ? "z" : "g") + entry, k -> {
            List<Instruction> out = new ArrayList<>();
            Function f = p.getFunctionManager().getFunctionAt(addr(p, entry));
            for (Instruction ins : p.getListing().getInstructions(f.getBody(), true)) {
                out.add(ins);
            }
            return out;
        });
    }

    /** Longest common subsequence of the instructions with every value masked. */
    private int[] align(long ze, long ge, List<Instruction> zl, List<Instruction> gl) {
        String key = ze + ":" + ge;
        if (alignments.containsKey(key)) {
            return alignments.get(key);
        }
        int n = zl.size(), m = gl.size();
        int[] out = null;
        if ((long) (n + 1) * (m + 1) <= 60_000_000L) {
            out = lcs(tokens(zh, zl), tokens(gen, gl));
        }
        alignments.put(key, out);
        return out;
    }

    /** For each x, the index in y it pairs with in a longest common subsequence, or -1. */
    private static int[] lcs(long[] x, long[] y) {
        int n = x.length, m = y.length;
        int[] dp = new int[(n + 1) * (m + 1)];
        for (int i = n - 1; i >= 0; i--) {
            for (int j = m - 1; j >= 0; j--) {
                dp[i * (m + 1) + j] = x[i] == y[j] ? dp[(i + 1) * (m + 1) + j + 1] + 1
                    : Math.max(dp[(i + 1) * (m + 1) + j], dp[i * (m + 1) + j + 1]);
            }
        }
        int[] out = new int[n];
        Arrays.fill(out, -1);
        for (int i = 0, j = 0; i < n && j < m;) {
            if (x[i] == y[j]) {
                out[i++] = j++;
            } else if (dp[(i + 1) * (m + 1) + j] >= dp[i * (m + 1) + j + 1]) {
                i++;
            } else {
                j++;
            }
        }
        return out;
    }

    private static long[] tokens(Program p, List<Instruction> l) {
        long lo = p.getMinAddress().getOffset(), hi = p.getMaxAddress().getOffset();
        long[] t = new long[l.size()];
        for (int i = 0; i < t.length; i++) {
            try {
                byte[] b = l.get(i).getBytes(), m = mask(l.get(i), true, lo, hi);
                long h = FNV;
                for (int k = 0; k < b.length; k++) {
                    h = (h ^ (b[k] & m[k] & 0xff)) * PRIME;
                }
                t[i] = h;
            } catch (Exception e) {
                t[i] = i;
            }
        }
        return t;
    }

    /** Data: find the nearest address at or below it that code refers to (a global, a vtable start),
     * map those instructions, and read the address they refer to in Generals. */
    private Mapped byReferences(Address a) {
        var refs = zh.getReferenceManager();
        for (int delta = 0; delta <= 0x800; delta++) {
            Address base = a.subtract(delta);
            if (!refs.hasReferencesTo(base)) {
                continue;
            }
            Map<Long, Integer> votes = new HashMap<>();
            int seen = 0;
            for (Reference r : refs.getReferencesTo(base)) {
                if (++seen > 300) {
                    break;
                }
                Function f = zh.getFunctionManager().getFunctionContaining(r.getFromAddress());
                Long ge = f == null ? null : z2g.get(f.getEntryPoint().getOffset());
                if (ge == null) {
                    continue;
                }
                Mapped gi = instruction(f.getEntryPoint().getOffset(), ge, r.getFromAddress());
                Instruction ins = gi.ok() ? gen.getListing().getInstructionAt(addr(gen, gi.gen())) : null;
                if (ins == null) {
                    continue;
                }
                for (Reference gr : ins.getReferencesFrom()) {
                    if (gr.getOperandIndex() == r.getOperandIndex()) {
                        votes.merge(gr.getToAddress().getOffset() + delta, 1, Integer::sum);
                    }
                }
            }
            if (votes.isEmpty()) {
                return new Mapped(-1, String.format("refs to %08x only from unmatched code", base.getOffset()));
            }
            var best = votes.entrySet().stream().max(Map.Entry.comparingByValue()).get();
            int total = votes.values().stream().mapToInt(Integer::intValue).sum();
            return new Mapped(best.getKey(), String.format("refs to %08x+%x, %d of %d agree", base.getOffset(), delta, best.getValue(), total));
        }
        return new Mapped(-1, "no refs");
    }

    /** A vtable entry or other pointer to a matched function: where Generals holds the matched one. */
    private Mapped byPointer(Address a) throws Exception {
        long v = zh.getMemory().getInt(a) & 0xffffffffL;
        Long g = z2g.get(v);
        if (g == null) {
            return new Mapped(-1, zh.getFunctionManager().getFunctionAt(addr(zh, v)) != null ? "points to unmatched function" : "not a pointer to a function");
        }
        byte[] want = {(byte) g.longValue(), (byte) (g >> 8), (byte) (g >> 16), (byte) (g >> 24)};
        List<Long> hits = new ArrayList<>();
        Address cur = gen.getMinAddress();
        while (cur != null && hits.size() < 8) {
            Address hit = gen.getMemory().findBytes(cur, want, null, true, monitor);
            if (hit == null) {
                break;
            }
            if (gen.getListing().getInstructionContaining(hit) == null) {
                hits.add(hit.getOffset());
            }
            cur = hit.add(1);
        }
        if (hits.size() == 1) {
            return new Mapped(hits.get(0), String.format("pointer to %08x", g));
        }
        StringBuilder sb = new StringBuilder(String.format("%d places hold %08x:", hits.size(), g));
        for (long h : hits) {
            sb.append(String.format(" %08x", h));
        }
        return new Mapped(-1, sb.toString());
    }
}
