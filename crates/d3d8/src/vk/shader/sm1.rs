//! Translates D3D8 shader bytecode (vs.1.x, ps.1.0 to ps.1.3) into WGSL.

use std::fmt::Write as _;

use super::{FRAGMENT_COMMON, VERTEX_OUT};

/// One vertex element of a vertex shader declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    pub stream: u32,
    pub offset: u32,
    /// D3DVSDT_* type.
    pub ty: u32,
    pub reg: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Declaration {
    pub elements: Vec<Element>,
    /// Constants the declaration loads when the shader is set.
    pub constants: Vec<(u32, [f32; 4])>,
}

pub const VSDT_FLOAT1: u32 = 0;
pub const VSDT_FLOAT2: u32 = 1;
pub const VSDT_FLOAT3: u32 = 2;
pub const VSDT_FLOAT4: u32 = 3;
pub const VSDT_D3DCOLOR: u32 = 4;
pub const VSDT_UBYTE4: u32 = 5;
pub const VSDT_SHORT2: u32 = 6;
pub const VSDT_SHORT4: u32 = 7;

pub fn element_size(ty: u32) -> u32 {
    match ty {
        VSDT_FLOAT1 | VSDT_D3DCOLOR | VSDT_UBYTE4 | VSDT_SHORT2 => 4,
        VSDT_FLOAT2 | VSDT_SHORT4 => 8,
        VSDT_FLOAT3 => 12,
        _ => 16,
    }
}

/// Reads a vertex shader declaration up to its end token. Returns it and its length in dwords.
pub fn parse_declaration(tokens: &[u32]) -> Result<(Declaration, usize), String> {
    let mut d = Declaration::default();
    let mut stream = 0;
    let mut offset = 0;
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        i += 1;
        if t == 0xFFFF_FFFF {
            return Ok((d, i));
        }
        match t >> 29 {
            0 => {}
            1 => {
                stream = t & 0xF;
                offset = 0;
            }
            2 => {
                if t & (1 << 28) != 0 {
                    offset += ((t >> 16) & 0xF) * 4;
                } else {
                    let ty = (t >> 16) & 0xF;
                    d.elements.push(Element { stream, offset, ty, reg: t & 0x1F });
                    offset += element_size(ty);
                }
            }
            4 => {
                let count = (t >> 25) & 0xF;
                let base = t & 0x7F;
                for k in 0..count {
                    let at = i + k as usize * 4;
                    let v = tokens.get(at..at + 4).ok_or("declaration ends inside a constant")?;
                    d.constants.push((base + k, [f32::from_bits(v[0]), f32::from_bits(v[1]), f32::from_bits(v[2]), f32::from_bits(v[3])]));
                }
                i += count as usize * 4;
            }
            5 => i += ((t >> 24) & 0x1F) as usize,
            other => return Err(format!("declaration token type {other} is not supported")),
        }
    }
    Err("declaration has no end token".into())
}

#[derive(Clone, Copy)]
struct Param {
    ty: u32,
    num: u32,
    mask: u32,
    result: u32,
    shift: u32,
    swizzle: u32,
    modifier: u32,
    relative: bool,
}

fn param(t: u32) -> Param {
    Param {
        ty: (t >> 28) & 7,
        num: t & 0x7FF,
        mask: (t >> 16) & 0xF,
        result: (t >> 20) & 0xF,
        shift: (t >> 24) & 0xF,
        swizzle: (t >> 16) & 0xFF,
        modifier: (t >> 24) & 0xF,
        relative: t & (1 << 13) != 0,
    }
}

struct Instruction {
    op: u32,
    dst: Option<Param>,
    src: Vec<Param>,
    def: [f32; 4],
}

const OP_NOP: u32 = 0;
const OP_MOV: u32 = 1;
const OP_ADD: u32 = 2;
const OP_SUB: u32 = 3;
const OP_MAD: u32 = 4;
const OP_MUL: u32 = 5;
const OP_RCP: u32 = 6;
const OP_RSQ: u32 = 7;
const OP_DP3: u32 = 8;
const OP_DP4: u32 = 9;
const OP_MIN: u32 = 10;
const OP_MAX: u32 = 11;
const OP_SLT: u32 = 12;
const OP_SGE: u32 = 13;
const OP_EXP: u32 = 14;
const OP_LOG: u32 = 15;
const OP_LIT: u32 = 16;
const OP_DST: u32 = 17;
const OP_LRP: u32 = 18;
const OP_FRC: u32 = 19;
const OP_M4X4: u32 = 20;
const OP_M4X3: u32 = 21;
const OP_M3X4: u32 = 22;
const OP_M3X3: u32 = 23;
const OP_M3X2: u32 = 24;
const OP_TEXCOORD: u32 = 64;
const OP_TEXKILL: u32 = 65;
const OP_TEX: u32 = 66;
const OP_TEXBEM: u32 = 67;
const OP_TEXBEML: u32 = 68;
const OP_TEXREG2AR: u32 = 69;
const OP_TEXREG2GB: u32 = 70;
const OP_TEXM3X2PAD: u32 = 71;
const OP_TEXM3X2TEX: u32 = 72;
const OP_EXPP: u32 = 78;
const OP_LOGP: u32 = 79;
const OP_CND: u32 = 80;
const OP_DEF: u32 = 81;
const OP_TEXREG2RGB: u32 = 82;
const OP_TEXDP3TEX: u32 = 83;
const OP_TEXDP3: u32 = 85;
const OP_CMP: u32 = 88;

fn source_count(op: u32) -> Option<usize> {
    Some(match op {
        OP_NOP => return Some(usize::MAX),
        OP_MOV | OP_RCP | OP_RSQ | OP_EXP | OP_LOG | OP_LIT | OP_FRC | OP_EXPP | OP_LOGP => 1,
        OP_ADD | OP_SUB | OP_MUL | OP_DP3 | OP_DP4 | OP_MIN | OP_MAX | OP_SLT | OP_SGE | OP_DST => 2,
        OP_M4X4 | OP_M4X3 | OP_M3X4 | OP_M3X3 | OP_M3X2 => 2,
        OP_MAD | OP_LRP | OP_CND | OP_CMP => 3,
        OP_TEXCOORD | OP_TEXKILL | OP_TEX => 0,
        OP_TEXBEM | OP_TEXBEML | OP_TEXREG2AR | OP_TEXREG2GB | OP_TEXM3X2PAD | OP_TEXM3X2TEX => 1,
        OP_TEXREG2RGB | OP_TEXDP3TEX | OP_TEXDP3 => 1,
        _ => return None,
    })
}

fn parse(tokens: &[u32]) -> Result<(u32, Vec<Instruction>), String> {
    let version = *tokens.first().ok_or("empty shader")?;
    let mut out = Vec::new();
    let mut i = 1;
    while i < tokens.len() {
        let t = tokens[i];
        i += 1;
        let op = t & 0xFFFF;
        if op == 0xFFFF {
            return Ok((version, out));
        }
        if op == 0xFFFE {
            i += ((t >> 16) & 0x7FFF) as usize;
            continue;
        }
        if op == OP_DEF {
            let v = tokens.get(i..i + 5).ok_or("shader ends inside def")?;
            out.push(Instruction {
                op,
                dst: Some(param(v[0])),
                src: Vec::new(),
                def: [f32::from_bits(v[1]), f32::from_bits(v[2]), f32::from_bits(v[3]), f32::from_bits(v[4])],
            });
            i += 5;
            continue;
        }
        let count = source_count(op).ok_or_else(|| format!("opcode {op} is not supported"))?;
        if count == usize::MAX {
            continue;
        }
        let dst = param(*tokens.get(i).ok_or("shader ends inside an instruction")?);
        i += 1;
        let mut src = Vec::new();
        for _ in 0..count {
            let s = *tokens.get(i).ok_or("shader ends inside an instruction")?;
            i += 1;
            src.push(param(s));
            // vs.1.1 relative addressing has no extra token, the address register is always a0.x.
        }
        out.push(Instruction { op, dst: Some(dst), src, def: [0.0; 4] });
    }
    Err("shader has no end token".into())
}

const COMPONENTS: [char; 4] = ['x', 'y', 'z', 'w'];

fn swizzle(sw: u32) -> String {
    if sw == 0xE4 {
        return String::new();
    }
    let mut s = String::from(".");
    for i in 0..4 {
        s.push(COMPONENTS[((sw >> (i * 2)) & 3) as usize]);
    }
    s
}

struct Writer {
    pixel: bool,
    body: String,
    defs: Vec<(u32, [f32; 4])>,
    temp: u32,
}

impl Writer {
    fn register(&self, p: &Param, offset: u32) -> Result<String, String> {
        let n = p.num + offset;
        Ok(match (p.ty, self.pixel) {
            (0, _) => format!("r{n}"),
            (1, false) => format!("v{n}"),
            (1, true) => format!("v{}", n.min(1)),
            (2, _) => {
                if !p.relative
                    && let Some((_, v)) = self.defs.iter().find(|(r, _)| *r == n)
                {
                    format!("vec4f({:?}, {:?}, {:?}, {:?})", v[0], v[1], v[2], v[3])
                } else if self.pixel {
                    format!("ps.c[{}]", n.min(7))
                } else if p.relative {
                    format!("vs.c[clamp(a0.x + {n}, 0, 95)]")
                } else {
                    format!("vs.c[{}]", n.min(95))
                }
            }
            (3, false) => "vec4f(a0)".into(),
            (3, true) => format!("t{n}"),
            (ty, _) => return Err(format!("source register type {ty} is not supported")),
        })
    }

    fn source(&self, p: &Param, offset: u32) -> Result<String, String> {
        let r = format!("{}{}", self.register(p, offset)?, swizzle(p.swizzle));
        Ok(match p.modifier {
            0 => r,
            1 => format!("(-{r})"),
            2 => format!("({r} - 0.5)"),
            3 => format!("(0.5 - {r})"),
            4 => format!("({r} * 2.0 - 1.0)"),
            5 => format!("(1.0 - {r} * 2.0)"),
            6 => format!("(1.0 - {r})"),
            7 => format!("({r} * 2.0)"),
            8 => format!("({r} * -2.0)"),
            m => return Err(format!("source modifier {m} is not supported")),
        })
    }

    fn dest_name(&self, p: &Param) -> Result<String, String> {
        Ok(match (p.ty, self.pixel) {
            (0, _) => format!("r{}", p.num),
            (3, false) => "a0".into(),
            (3, true) => format!("t{}", p.num),
            (4, false) => ["opos", "ofog", "opts"].get(p.num as usize).ok_or("bad rasterizer output")?.to_string(),
            (5, false) => format!("od{}", p.num.min(1)),
            (6, false) => format!("ot{}", p.num.min(7)),
            (ty, _) => return Err(format!("destination register type {ty} is not supported")),
        })
    }

    /// Writes `value` (a vec4f expression) to the destination, honoring its mask and modifiers.
    fn write(&mut self, p: &Param, value: String) -> Result<(), String> {
        let name = self.dest_name(p)?;
        let mut v = value;
        let scale = match p.shift {
            0 => None,
            1 => Some("2.0"),
            2 => Some("4.0"),
            3 => Some("8.0"),
            13 => Some("0.125"),
            14 => Some("0.25"),
            15 => Some("0.5"),
            s => return Err(format!("result shift {s} is not supported")),
        };
        if let Some(scale) = scale {
            v = format!("({v} * {scale})");
        }
        if p.result & 1 != 0 {
            v = format!("saturate({v})");
        }
        let t = self.temp;
        self.temp += 1;
        if name == "a0" {
            let _ = writeln!(self.body, "    let _t{t} = {v};\n    a0 = vec4i(floor(_t{t}));");
            return Ok(());
        }
        let _ = writeln!(self.body, "    let _t{t} = {v};");
        let mask = if p.mask == 0 { 0xF } else { p.mask };
        for (i, c) in COMPONENTS.iter().enumerate() {
            if mask & (1 << i) != 0 {
                let _ = writeln!(self.body, "    {name}.{c} = _t{t}.{c};");
            }
        }
        Ok(())
    }

    fn texture(&self, stage: u32, coords: &str) -> String {
        format!("select(vec4f(0.0, 0.0, 0.0, 1.0), textureSample(tex{stage}, smp{stage}, {coords}), ps.stages[{stage}].misc.y != 0u)")
    }

    fn instruction(&mut self, ins: &Instruction) -> Result<(), String> {
        let dst = ins.dst.as_ref().unwrap();
        let s = |w: &Writer, i: usize| w.source(&ins.src[i], 0);
        let value = match ins.op {
            OP_DEF => {
                self.defs.push((dst.num, ins.def));
                return Ok(());
            }
            OP_MOV => s(self, 0)?,
            OP_ADD => format!("({} + {})", s(self, 0)?, s(self, 1)?),
            OP_SUB => format!("({} - {})", s(self, 0)?, s(self, 1)?),
            OP_MUL => format!("({} * {})", s(self, 0)?, s(self, 1)?),
            OP_MAD => format!("({} * {} + {})", s(self, 0)?, s(self, 1)?, s(self, 2)?),
            OP_RCP => format!("vec4f(1.0 / {}.w)", s(self, 0)?),
            OP_RSQ => format!("vec4f(inverseSqrt(abs({}.w)))", s(self, 0)?),
            OP_DP3 => format!("vec4f(dot({}.xyz, {}.xyz))", s(self, 0)?, s(self, 1)?),
            OP_DP4 => format!("vec4f(dot({}, {}))", s(self, 0)?, s(self, 1)?),
            OP_MIN => format!("min({}, {})", s(self, 0)?, s(self, 1)?),
            OP_MAX => format!("max({}, {})", s(self, 0)?, s(self, 1)?),
            OP_SLT => format!("select(vec4f(0.0), vec4f(1.0), {} < {})", s(self, 0)?, s(self, 1)?),
            OP_SGE => format!("select(vec4f(0.0), vec4f(1.0), {} >= {})", s(self, 0)?, s(self, 1)?),
            OP_EXP => format!("vec4f(exp2({}.w))", s(self, 0)?),
            OP_EXPP => {
                let a = s(self, 0)?;
                format!("vec4f(exp2(floor({a}.w)), fract({a}.w), exp2({a}.w), 1.0)")
            }
            OP_LOG | OP_LOGP => format!("vec4f(log2(abs({}.w)))", s(self, 0)?),
            OP_LIT => {
                let a = s(self, 0)?;
                format!(
                    "vec4f(1.0, max({a}.x, 0.0), select(0.0, pow(max({a}.y, 0.0), clamp({a}.w, -128.0, 128.0)), {a}.x > 0.0), 1.0)"
                )
            }
            OP_DST => {
                let (a, b) = (s(self, 0)?, s(self, 1)?);
                format!("vec4f(1.0, {a}.y * {b}.y, {a}.z, {b}.w)")
            }
            OP_LRP => {
                let (a, b, c) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                format!("mix({c}, {b}, {a})")
            }
            OP_FRC => format!("fract({})", s(self, 0)?),
            OP_M4X4 | OP_M4X3 | OP_M3X4 | OP_M3X3 | OP_M3X2 => {
                let (rows, three) = match ins.op {
                    OP_M4X4 => (4, false),
                    OP_M4X3 => (3, false),
                    OP_M3X4 => (4, true),
                    OP_M3X3 => (3, true),
                    _ => (2, true),
                };
                let a = s(self, 0)?;
                let mut parts = Vec::new();
                for row in 0..4 {
                    if row < rows {
                        let m = self.source(&ins.src[1], row)?;
                        parts.push(if three { format!("dot({a}.xyz, {m}.xyz)") } else { format!("dot({a}, {m})") });
                    } else {
                        parts.push("0.0".into());
                    }
                }
                format!("vec4f({})", parts.join(", "))
            }
            OP_CND => {
                let (a, b, c) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                format!("select({c}, {b}, {a}.w > 0.5)")
            }
            OP_CMP => {
                let (a, b, c) = (s(self, 0)?, s(self, 1)?, s(self, 2)?);
                format!("select({c}, {b}, {a} >= vec4f(0.0))")
            }
            OP_TEX => {
                let n = dst.num;
                self.texture(n, &format!("project(input.t{n}, ps.stages[{n}].misc.z).xy"))
            }
            OP_TEXCOORD => format!("vec4f(saturate(input.t{}.xyz), 1.0)", dst.num),
            OP_TEXKILL => {
                let n = dst.num;
                let what = if dst.ty == 3 { format!("input.t{n}") } else { self.dest_name(dst)? };
                let _ = writeln!(self.body, "    if (any({what}.xyz < vec3f(0.0))) {{\n        discard;\n    }}");
                return Ok(());
            }
            OP_TEXBEM | OP_TEXBEML => {
                let (m, src) = (dst.num, s(self, 0)?);
                let coords = format!(
                    "(project(input.t{m}, ps.stages[{m}].misc.z).xy + vec2f(ps.stages[{m}].bump.x * {src}.x + ps.stages[{m}].bump.z * {src}.y, ps.stages[{m}].bump.y * {src}.x + ps.stages[{m}].bump.w * {src}.y))"
                );
                let sample = self.texture(m, &coords);
                if ins.op == OP_TEXBEML {
                    format!(
                        "({sample} * vec4f(vec3f(saturate({src}.z * ps.stages[{m}].bumpl.x + ps.stages[{m}].bumpl.y)), 1.0))"
                    )
                } else {
                    sample
                }
            }
            OP_TEXREG2AR => {
                let src = s(self, 0)?;
                self.texture(dst.num, &format!("{src}.wx"))
            }
            OP_TEXREG2GB => {
                let src = s(self, 0)?;
                self.texture(dst.num, &format!("{src}.yz"))
            }
            OP_TEXREG2RGB => {
                let src = s(self, 0)?;
                self.texture(dst.num, &format!("{src}.xy"))
            }
            OP_TEXM3X2PAD => {
                let m = dst.num;
                format!("vec4f(dot(input.t{m}.xyz, {}.xyz))", s(self, 0)?)
            }
            OP_TEXM3X2TEX => {
                let m = dst.num;
                let src = s(self, 0)?;
                self.texture(m, &format!("vec2f(t{}.x, dot(input.t{m}.xyz, {src}.xyz))", m.saturating_sub(1)))
            }
            OP_TEXDP3TEX => {
                let m = dst.num;
                self.texture(m, &format!("vec2f(dot(input.t{m}.xyz, {}.xyz), 0.0)", s(self, 0)?))
            }
            OP_TEXDP3 => format!("vec4f(dot(input.t{}.xyz, {}.xyz))", dst.num, s(self, 0)?),
            op => return Err(format!("opcode {op} is not supported")),
        };
        self.write(dst, value)
    }
}

/// A vertex shader in WGSL. Inputs are the declared registers at locations 0 to 15.
pub fn translate_vertex(function: &[u32], decl: &Declaration) -> Result<String, String> {
    let (version, instructions) = parse(function)?;
    if version >> 16 != 0xFFFE || (version >> 8) & 0xFF != 1 {
        return Err(format!("vertex shader version {version:#x} is not supported"));
    }
    let mut w = Writer { pixel: false, body: String::new(), defs: Vec::new(), temp: 0 };
    for ins in &instructions {
        w.instruction(ins)?;
    }
    let mut s = String::from(VERTEX_OUT);
    s.push_str(
        "
struct VsC {
    viewport: vec4f,
    depth: vec4f,
    c: array<vec4f, 96>,
}

@group(0) @binding(0) var<uniform> vs: VsC;

@vertex
fn main(",
    );
    let mut regs: Vec<&Element> = decl.elements.iter().collect();
    regs.sort_by_key(|e| e.reg);
    regs.dedup_by_key(|e| e.reg);
    for e in &regs {
        let _ = write!(s, "@location({}) in_v{}: vec4f, ", e.reg, e.reg);
    }
    s.push_str(") -> Out {\n    var o: Out;\n");
    for r in 0..16 {
        match regs.iter().find(|e| e.reg == r) {
            Some(e) if e.ty == VSDT_D3DCOLOR => {
                let _ = writeln!(s, "    let v{r} = in_v{r}.zyxw;");
            }
            Some(_) => {
                let _ = writeln!(s, "    let v{r} = in_v{r};");
            }
            None => {
                let _ = writeln!(s, "    let v{r} = vec4f(0.0, 0.0, 0.0, 1.0);");
            }
        }
    }
    for r in 0..12 {
        let _ = writeln!(s, "    var r{r} = vec4f(0.0);");
    }
    s.push_str("    var a0 = vec4i(0);\n    var opos = vec4f(0.0, 0.0, 0.0, 1.0);\n    var ofog = vec4f(0.0);\n    var opts = vec4f(0.0);\n");
    s.push_str("    var od0 = vec4f(1.0);\n    var od1 = vec4f(0.0);\n");
    for t in 0..8 {
        let _ = writeln!(s, "    var ot{t} = vec4f(0.0, 0.0, 0.0, 1.0);");
    }
    s.push_str(&w.body);
    s.push_str(
        "    o.pos = to_vulkan(vec4f(opos.xy, opos.w - opos.z, opos.w), vs.viewport);
    o.pos.z += vs.depth.z * o.pos.w;
    o.clip = array<f32, 6>(1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
    o.d0 = saturate(od0);
    o.d1 = saturate(od1);
    o.d0f = o.d0;
    o.d1f = o.d1;
    o.t0 = ot0;
    o.t1 = ot1;
    o.t2 = ot2;
    o.t3 = ot3;
    o.t4 = ot4;
    o.t5 = ot5;
    o.t6 = ot6;
    o.t7 = ot7;
    return o;
}
",
    );
    Ok(s)
}

/// A pixel shader in WGSL. Returns the source and the shader version.
pub fn translate_pixel(function: &[u32]) -> Result<(String, u32), String> {
    let (version, instructions) = parse(function)?;
    let (major, minor) = ((version >> 8) & 0xFF, version & 0xFF);
    if version >> 16 != 0xFFFF || major != 1 || minor > 3 {
        return Err(format!("pixel shader version {major}.{minor} is not supported"));
    }
    let mut w = Writer { pixel: true, body: String::new(), defs: Vec::new(), temp: 0 };
    for ins in &instructions {
        w.instruction(ins)?;
    }
    let mut s = String::from(FRAGMENT_COMMON);
    s.push_str(
        "
@fragment
fn main(input: In) -> @location(0) vec4f {
    var v0 = input.d0;
    var v1 = input.d1;
    if (ps.flags.z != 0u) {
        v0 = input.d0f;
        v1 = input.d1f;
    }
    var r0 = vec4f(0.0);
    var r1 = vec4f(0.0);
    var t0 = vec4f(0.0);
    var t1 = vec4f(0.0);
    var t2 = vec4f(0.0);
    var t3 = vec4f(0.0);
",
    );
    s.push_str(&w.body);
    s.push_str(
        "    let color = saturate(r0);
    if (!passes_alpha_test(color)) {
        discard;
    }
    return color;
}
",
    );
    Ok((s, version))
}
