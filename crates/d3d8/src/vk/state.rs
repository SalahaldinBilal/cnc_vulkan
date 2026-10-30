//! The D3D8 render state the game sets, and the uniform blocks the shaders read it from.

use super::shader::{CLIP_PLANES, ff};

pub const RS_ZENABLE: usize = 7;
pub const RS_FILLMODE: usize = 8;
pub const RS_SHADEMODE: usize = 9;
pub const RS_ZWRITEENABLE: usize = 14;
pub const RS_ALPHATESTENABLE: usize = 15;
pub const RS_SRCBLEND: usize = 19;
pub const RS_DESTBLEND: usize = 20;
pub const RS_CULLMODE: usize = 22;
pub const RS_ZFUNC: usize = 23;
pub const RS_ALPHAREF: usize = 24;
pub const RS_ALPHAFUNC: usize = 25;
pub const RS_ALPHABLENDENABLE: usize = 27;
pub const RS_FOGENABLE: usize = 28;
pub const RS_SPECULARENABLE: usize = 29;
pub const RS_ZBIAS: usize = 47;
pub const RS_STENCILENABLE: usize = 52;
pub const RS_STENCILFAIL: usize = 53;
pub const RS_STENCILZFAIL: usize = 54;
pub const RS_STENCILPASS: usize = 55;
pub const RS_STENCILFUNC: usize = 56;
pub const RS_STENCILREF: usize = 57;
pub const RS_STENCILMASK: usize = 58;
pub const RS_STENCILWRITEMASK: usize = 59;
pub const RS_TEXTUREFACTOR: usize = 60;
pub const RS_LIGHTING: usize = 137;
pub const RS_AMBIENT: usize = 139;
pub const RS_COLORVERTEX: usize = 141;
pub const RS_LOCALVIEWER: usize = 142;
pub const RS_NORMALIZENORMALS: usize = 143;
pub const RS_DIFFUSEMATERIALSOURCE: usize = 145;
pub const RS_SPECULARMATERIALSOURCE: usize = 146;
pub const RS_AMBIENTMATERIALSOURCE: usize = 147;
pub const RS_EMISSIVEMATERIALSOURCE: usize = 148;
pub const RS_CLIPPLANEENABLE: usize = 152;
pub const RS_COLORWRITEENABLE: usize = 168;
pub const RS_BLENDOP: usize = 171;

pub const TSS_COLOROP: usize = 1;
pub const TSS_COLORARG1: usize = 2;
pub const TSS_COLORARG2: usize = 3;
pub const TSS_ALPHAOP: usize = 4;
pub const TSS_ALPHAARG1: usize = 5;
pub const TSS_ALPHAARG2: usize = 6;
pub const TSS_BUMPENVMAT00: usize = 7;
pub const TSS_TEXCOORDINDEX: usize = 11;
pub const TSS_ADDRESSU: usize = 13;
pub const TSS_ADDRESSV: usize = 14;
pub const TSS_BORDERCOLOR: usize = 15;
pub const TSS_MAGFILTER: usize = 16;
pub const TSS_MINFILTER: usize = 17;
pub const TSS_MIPFILTER: usize = 18;
pub const TSS_MIPMAPLODBIAS: usize = 19;
pub const TSS_MAXMIPLEVEL: usize = 20;
pub const TSS_MAXANISOTROPY: usize = 21;
pub const TSS_BUMPENVLSCALE: usize = 22;
pub const TSS_BUMPENVLOFFSET: usize = 23;
pub const TSS_TEXTURETRANSFORMFLAGS: usize = 24;
pub const TSS_ADDRESSW: usize = 25;
pub const TSS_COLORARG0: usize = 26;
pub const TSS_ALPHAARG0: usize = 27;
pub const TSS_RESULTARG: usize = 28;

pub const TS_VIEW: u32 = 2;
pub const TS_PROJECTION: u32 = 3;
pub const TS_TEXTURE0: u32 = 16;
pub const TS_WORLD: u32 = 256;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f32; 16]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    /// Row-vector product, as D3D composes transforms: `self` first, then `other`.
    pub fn mul(&self, other: &Matrix) -> Matrix {
        let (a, b) = (&self.0, &other.0);
        let mut m = [0.0f32; 16];
        for i in 0..4 {
            for j in 0..4 {
                m[i * 4 + j] = (0..4).map(|k| a[i * 4 + k] * b[k * 4 + j]).sum();
            }
        }
        Matrix(m)
    }

    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12],
            p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13],
            p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14],
        ]
    }

    pub fn transform_vector(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            v[0] * m[0] + v[1] * m[4] + v[2] * m[8],
            v[0] * m[1] + v[1] * m[5] + v[2] * m[9],
            v[0] * m[2] + v[1] * m[6] + v[2] * m[10],
        ]
    }

    /// This projection with depth reversed: clip z becomes w - z. Built into the matrix rather
    /// than subtracted afterwards, so far depths keep the float precision near 0.
    pub fn reversed_depth(&self) -> Matrix {
        let mut m = self.0;
        for row in 0..4 {
            m[row * 4 + 2] = self.0[row * 4 + 3] - self.0[row * 4 + 2];
        }
        Matrix(m)
    }

    /// The inverse transpose of the upper 3x3, which carries normals through this transform.
    pub fn normal_matrix(&self) -> Matrix {
        let m = &self.0;
        let (a, b, c, d, e, f, g, h, i) = (m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]);
        let co = [e * i - f * h, -(d * i - f * g), d * h - e * g, -(b * i - c * h), a * i - c * g, -(a * h - b * g), b * f - c * e, -(a * f - c * d), a * e - b * d];
        let det = a * co[0] + b * co[1] + c * co[2];
        let s = if det.abs() < 1e-30 { 0.0 } else { 1.0 / det };
        // The inverse transpose is the cofactor matrix over the determinant.
        Matrix([
            co[0] * s, co[1] * s, co[2] * s, 0.0,
            co[3] * s, co[4] * s, co[5] * s, 0.0,
            co[6] * s, co[7] * s, co[8] * s, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ])
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Light {
    pub ty: u32,
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub ambient: [f32; 4],
    pub position: [f32; 3],
    pub direction: [f32; 3],
    pub range: f32,
    pub falloff: f32,
    pub attenuation0: f32,
    pub attenuation1: f32,
    pub attenuation2: f32,
    pub theta: f32,
    pub phi: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Material {
    pub diffuse: [f32; 4],
    pub ambient: [f32; 4],
    pub specular: [f32; 4],
    pub emissive: [f32; 4],
    pub power: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub min_z: f32,
    pub max_z: f32,
}

#[derive(Clone)]
pub struct State {
    pub rs: [u32; 256],
    pub tss: [[u32; 32]; 8],
    pub world: Matrix,
    pub view: Matrix,
    pub proj: Matrix,
    pub tex: [Matrix; 8],
    pub viewport: Viewport,
    pub material: Material,
    pub lights: Vec<Light>,
    /// Light indices in the order they were enabled.
    pub enabled_lights: Vec<u32>,
    pub clip_planes: [[f32; 4]; 32],
    pub vs_const: [[f32; 4]; 96],
    pub ps_const: [[f32; 4]; 8],
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

impl State {
    pub fn new(depth: bool) -> State {
        let mut rs = [0u32; 256];
        let defaults: &[(usize, u32)] = &[
            (RS_ZENABLE, depth as u32),
            (RS_FILLMODE, 3),
            (RS_SHADEMODE, 2),
            (RS_ZWRITEENABLE, 1),
            (16, 1),
            (RS_SRCBLEND, 2),
            (RS_DESTBLEND, 1),
            (RS_CULLMODE, 3),
            (RS_ZFUNC, 4),
            (RS_ALPHAFUNC, 8),
            (37, f(1.0)),
            (38, f(1.0)),
            (RS_STENCILFAIL, 1),
            (RS_STENCILZFAIL, 1),
            (RS_STENCILPASS, 1),
            (RS_STENCILFUNC, 8),
            (RS_STENCILMASK, 0xFFFF_FFFF),
            (RS_STENCILWRITEMASK, 0xFFFF_FFFF),
            (RS_TEXTUREFACTOR, 0xFFFF_FFFF),
            (136, 1),
            (RS_LIGHTING, 1),
            (RS_COLORVERTEX, 1),
            (RS_LOCALVIEWER, 1),
            (RS_DIFFUSEMATERIALSOURCE, 1),
            (RS_SPECULARMATERIALSOURCE, 2),
            (154, f(1.0)),
            (155, f(1.0)),
            (158, f(1.0)),
            (161, 1),
            (162, 0xFFFF_FFFF),
            (164, f(1.0)),
            (166, f(64.0)),
            (RS_COLORWRITEENABLE, 0xF),
            (RS_BLENDOP, 1),
        ];
        for &(i, v) in defaults {
            rs[i] = v;
        }
        let mut tss = [[0u32; 32]; 8];
        for (stage, t) in tss.iter_mut().enumerate() {
            t[TSS_COLOROP] = if stage == 0 { 4 } else { 1 };
            t[TSS_COLORARG1] = 2;
            t[TSS_COLORARG2] = 1;
            t[TSS_ALPHAOP] = if stage == 0 { 2 } else { 1 };
            t[TSS_ALPHAARG1] = 2;
            t[TSS_ALPHAARG2] = 1;
            t[TSS_TEXCOORDINDEX] = stage as u32;
            t[TSS_ADDRESSU] = 1;
            t[TSS_ADDRESSV] = 1;
            t[TSS_ADDRESSW] = 1;
            t[TSS_MAGFILTER] = 1;
            t[TSS_MINFILTER] = 1;
            t[TSS_MAXANISOTROPY] = 1;
            t[TSS_COLORARG0] = 1;
            t[TSS_ALPHAARG0] = 1;
            t[TSS_RESULTARG] = 1;
        }
        State {
            rs,
            tss,
            world: Matrix::IDENTITY,
            view: Matrix::IDENTITY,
            proj: Matrix::IDENTITY,
            tex: [Matrix::IDENTITY; 8],
            viewport: Viewport::default(),
            material: Material::default(),
            lights: Vec::new(),
            enabled_lights: Vec::new(),
            clip_planes: [[0.0; 4]; 32],
            vs_const: [[0.0; 4]; 96],
            ps_const: [[0.0; 4]; 8],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LightBlock {
    diffuse: [f32; 4],
    specular: [f32; 4],
    ambient: [f32; 4],
    position: [f32; 4],
    direction: [f32; 4],
    atten: [f32; 4],
    spot: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VsBlock {
    proj: Matrix,
    tex: [Matrix; 8],
    mat_diffuse: [f32; 4],
    mat_ambient: [f32; 4],
    mat_specular: [f32; 4],
    mat_emissive: [f32; 4],
    ambient: [f32; 4],
    misc: [f32; 4],
    lights: [LightBlock; 8],
    tci: [[u32; 4]; 8],
    flags: [u32; 4],
    viewport: [f32; 4],
    depth: [f32; 4],
    clip: [[f32; 4]; CLIP_PLANES],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VsShaderBlock {
    viewport: [f32; 4],
    depth: [f32; 4],
    c: [[f32; 4]; 96],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct StageBlock {
    color: [u32; 4],
    alpha: [u32; 4],
    misc: [u32; 4],
    bump: [f32; 4],
    bumpl: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PsBlock {
    stages: [StageBlock; 8],
    tfactor: [f32; 4],
    flags: [u32; 4],
    c: [[f32; 4]; 8],
    count: [u32; 4],
    soft: [f32; 4],
}

/// Space reserved for the vertex uniforms, enough for either kind of vertex shader.
pub const VS_BLOCK_SIZE: u64 = {
    let a = size_of::<VsBlock>();
    let b = size_of::<VsShaderBlock>();
    (if a > b { a } else { b }) as u64
};
pub const PS_BLOCK_SIZE: u64 = size_of::<PsBlock>() as u64;

fn color(c: u32) -> [f32; 4] {
    [
        ((c >> 16) & 0xFF) as f32 / 255.0,
        ((c >> 8) & 0xFF) as f32 / 255.0,
        (c & 0xFF) as f32 / 255.0,
        (c >> 24) as f32 / 255.0,
    ]
}

/// D3D8's ZBIAS steps, as a fraction of the depth range (D3D9's DEPTHBIAS counts them the same).
const ZBIAS_STEP: f32 = 0.000005;

/// Viewport position and size, then depth range and the ZBIAS offset in normalized depth.
/// The offset is applied in the vertex shader, since a float depth buffer would scale a
/// hardware depth bias by the depth value.
fn viewport_vec(s: &State) -> ([f32; 4], [f32; 4]) {
    let v = &s.viewport;
    let range = v.max_z - v.min_z;
    let bias = if range > 0.0 { s.rs[RS_ZBIAS] as f32 * ZBIAS_STEP / range } else { 0.0 };
    (
        [v.x as f32, v.y as f32, v.width.max(1) as f32, v.height.max(1) as f32],
        [v.min_z, v.max_z, bias, 0.0],
    )
}

/// What the fixed function vertex stage needs to know about the vertex data.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct VertexInfo {
    pub rhw: bool,
    pub diffuse: bool,
    pub specular: bool,
    /// Components in each texture coordinate set.
    pub tex_dims: [u32; 8],
}

/// The fixed function vertex shader's push constants: world times view, and its normal matrix.
pub fn object_matrices(s: &State) -> [Matrix; 2] {
    let wv = s.world.mul(&s.view);
    [wv, wv.normal_matrix()]
}

pub const OBJECT_MATRICES_SIZE: u32 = size_of::<[Matrix; 2]>() as u32;

/// Whether a box (min x, y, z, max x, y, z) carried to D3D clip space by `m` lies wholly on the
/// outer side of one clip plane, so nothing of it can be rasterized. The planes get a little slack
/// for the half pixel shift and ZBIAS the vertex shader adds.
pub fn box_outside_clip(m: &Matrix, b: &[f32; 6]) -> bool {
    let m = &m.0;
    // Left, right, bottom, top, near, far.
    let mut outside = [true; 6];
    for corner in 0..8 {
        let x = b[if corner & 1 == 0 { 0 } else { 3 }];
        let y = b[if corner & 2 == 0 { 1 } else { 4 }];
        let z = b[if corner & 4 == 0 { 2 } else { 5 }];
        let cx = x * m[0] + y * m[4] + z * m[8] + m[12];
        let cy = x * m[1] + y * m[5] + z * m[9] + m[13];
        let cz = x * m[2] + y * m[6] + z * m[10] + m[14];
        let cw = x * m[3] + y * m[7] + z * m[11] + m[15];
        let slack = cw.abs() * 0.01 + 1e-4;
        outside[0] &= cx < -cw - slack;
        outside[1] &= cx > cw + slack;
        outside[2] &= cy < -cw - slack;
        outside[3] &= cy > cw + slack;
        outside[4] &= cz < -slack;
        outside[5] &= cz > cw + slack;
        if !outside.contains(&true) {
            return false;
        }
    }
    true
}

/// The enabled lights in view space, kept until the view or an enabled light changes. The vertex
/// block is rebuilt hundreds of times a frame, for materials and vertex formats, with the same lights.
#[derive(Default)]
pub struct LightCache {
    key: Vec<u8>,
    scratch: Vec<u8>,
    lights: [LightBlock; 8],
    count: usize,
}

impl LightCache {
    fn get(&mut self, s: &State) -> ([LightBlock; 8], usize) {
        self.scratch.clear();
        self.scratch.extend_from_slice(as_bytes(&s.view));
        for &index in s.enabled_lights.iter().take(8) {
            self.scratch.extend_from_slice(&index.to_ne_bytes());
            if let Some(l) = s.lights.get(index as usize) {
                self.scratch.extend_from_slice(as_bytes(l));
            }
        }
        if !bytes_equal(&self.key, &self.scratch) {
            (self.lights, self.count) = light_blocks(s);
            std::mem::swap(&mut self.key, &mut self.scratch);
        }
        (self.lights, self.count)
    }
}

fn light_blocks(s: &State) -> ([LightBlock; 8], usize) {
    let mut lights = [LightBlock::default(); 8];
    let mut count = 0;
    for &index in s.enabled_lights.iter().take(8) {
        let Some(l) = s.lights.get(index as usize) else { continue };
        let p = s.view.transform_point(l.position);
        let d = s.view.transform_vector(l.direction);
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let d = if len > 0.0 { [d[0] / len, d[1] / len, d[2] / len] } else { d };
        lights[count] = LightBlock {
            diffuse: l.diffuse,
            specular: l.specular,
            ambient: l.ambient,
            position: [p[0], p[1], p[2], 1.0],
            direction: [d[0], d[1], d[2], 0.0],
            atten: [l.range, l.attenuation0, l.attenuation1, l.attenuation2],
            spot: if l.ty == 2 { [(l.theta * 0.5).cos(), (l.phi * 0.5).cos(), l.falloff, 2.0] } else { [0.0, 0.0, l.falloff, l.ty as f32] },
        };
        count += 1;
    }
    (lights, count)
}

/// The enabled user clip planes moved from world space, as D3D takes them, into view space.
fn clip_planes(s: &State, rhw: bool) -> (u32, [[f32; 4]; CLIP_PLANES]) {
    let mut planes = [[0.0; 4]; CLIP_PLANES];
    let mask = if rhw { 0 } else { s.rs[RS_CLIPPLANEENABLE] & ((1 << CLIP_PLANES) - 1) };
    if mask == 0 {
        return (0, planes);
    }
    let inverse_transpose = s.view.normal_matrix();
    let t = [s.view.0[12], s.view.0[13], s.view.0[14]];
    for (i, out) in planes.iter_mut().enumerate().filter(|(i, _)| (mask >> i) & 1 != 0) {
        let p = s.clip_planes[i];
        let n = inverse_transpose.transform_vector([p[0], p[1], p[2]]);
        *out = [n[0], n[1], n[2], p[3] - (t[0] * n[0] + t[1] * n[1] + t[2] * n[2])];
    }
    (mask, planes)
}

pub fn vs_block(s: &State, vi: &VertexInfo, light_cache: &mut LightCache) -> VsBlock {
    let (lights, count) = light_cache.get(s);
    let mut tci = [[0u32; 4]; 8];
    for (i, t) in tci.iter_mut().enumerate() {
        let index = s.tss[i][TSS_TEXCOORDINDEX];
        let set = index & 0xFFFF;
        *t = [set, (index >> 16) & 0xF, s.tss[i][TSS_TEXTURETRANSFORMFLAGS], vi.tex_dims.get(set as usize).copied().unwrap_or(2)];
    }
    let rs = &s.rs;
    let mut flags = 0;
    if rs[RS_LIGHTING] != 0 {
        flags |= ff::F_LIGHTING;
    }
    if vi.rhw {
        flags |= ff::F_RHW;
    }
    if rs[RS_NORMALIZENORMALS] != 0 {
        flags |= ff::F_NORMALIZE;
    }
    if rs[RS_LOCALVIEWER] != 0 {
        flags |= ff::F_LOCAL_VIEWER;
    }
    if rs[RS_SPECULARENABLE] != 0 {
        flags |= ff::F_SPECULAR;
    }
    if rs[RS_COLORVERTEX] != 0 {
        flags |= ff::F_COLOR_VERTEX;
    }
    if vi.diffuse {
        flags |= ff::F_HAS_DIFFUSE;
    }
    if vi.specular {
        flags |= ff::F_HAS_SPECULAR;
    }
    let sources = (rs[RS_DIFFUSEMATERIALSOURCE] & 3)
        | (rs[RS_AMBIENTMATERIALSOURCE] & 3) << 2
        | (rs[RS_SPECULARMATERIALSOURCE] & 3) << 4
        | (rs[RS_EMISSIVEMATERIALSOURCE] & 3) << 6;
    let (viewport, depth) = viewport_vec(s);
    let (clip_mask, clip) = clip_planes(s, vi.rhw);
    let m = &s.material;
    VsBlock {
        proj: s.proj.reversed_depth(),
        tex: s.tex,
        mat_diffuse: m.diffuse,
        mat_ambient: m.ambient,
        mat_specular: m.specular,
        mat_emissive: m.emissive,
        ambient: color(rs[RS_AMBIENT]),
        misc: [m.power, 0.0, 0.0, 0.0],
        lights,
        tci,
        flags: [flags, count as u32, sources, clip_mask],
        viewport,
        depth,
        clip,
    }
}

pub fn vs_shader_block(s: &State) -> VsShaderBlock {
    let (viewport, depth) = viewport_vec(s);
    VsShaderBlock { viewport, depth, c: s.vs_const }
}

/// An operation that reads the texture of a stage without one passes the current color through,
/// as D3D does (Wine's `is_invalid_op` matches Windows here).
fn pass_through_without_texture(op: [u32; 4], has_texture: bool) -> [u32; 4] {
    const DISABLE: u32 = 1;
    const SELECTARG1: u32 = 2;
    const SELECTARG2: u32 = 3;
    const MULTIPLYADD: u32 = 25;
    const LERP: u32 = 26;
    const TEXTURE: u32 = 2;
    const CURRENT: u32 = 1;
    let [o, a1, a2, a0] = op;
    if has_texture || o == DISABLE {
        return op;
    }
    let reads = |a: u32| a & 0xF == TEXTURE;
    let invalid = (reads(a1) && o != SELECTARG2) || (reads(a2) && o != SELECTARG1) || (reads(a0) && (o == MULTIPLYADD || o == LERP));
    if invalid { [SELECTARG1, CURRENT, a2, a0] } else { op }
}

/// Fixed function stages up to the first disabled one. Stages after it are never sampled.
pub fn active_stages(s: &State) -> usize {
    s.tss.iter().take(8).position(|t| t[TSS_COLOROP] == 1).unwrap_or(8)
}

/// `bound` has a bit set for each stage with a texture.
/// `soft` is the soft particle setup the fixed function fragment shader reads, zero for none.
pub fn ps_block(s: &State, bound: u32, fixed_function: bool, soft: [f32; 4]) -> PsBlock {
    let mut stages = [StageBlock::default(); 8];
    let mut active = true;
    let mut count = 0;
    for (i, st) in stages.iter_mut().enumerate() {
        let t = &s.tss[i];
        if t[TSS_COLOROP] == 1 {
            active = false;
        }
        if active {
            count += 1;
        }
        let has_texture = (bound >> i) & 1 != 0;
        st.color = pass_through_without_texture([t[TSS_COLOROP], t[TSS_COLORARG1], t[TSS_COLORARG2], t[TSS_COLORARG0]], has_texture);
        st.alpha = pass_through_without_texture([t[TSS_ALPHAOP], t[TSS_ALPHAARG1], t[TSS_ALPHAARG2], t[TSS_ALPHAARG0]], has_texture);
        st.misc = [t[TSS_RESULTARG] & 0xF, (bound >> i) & 1, t[TSS_TEXTURETRANSFORMFLAGS], active as u32];
        st.bump = [
            f32::from_bits(t[TSS_BUMPENVMAT00]),
            f32::from_bits(t[TSS_BUMPENVMAT00 + 1]),
            f32::from_bits(t[TSS_BUMPENVMAT00 + 2]),
            f32::from_bits(t[TSS_BUMPENVMAT00 + 3]),
        ];
        st.bumpl = [f32::from_bits(t[TSS_BUMPENVLSCALE]), f32::from_bits(t[TSS_BUMPENVLOFFSET]), 0.0, 0.0];
    }
    let rs = &s.rs;
    let alpha_func = if rs[RS_ALPHATESTENABLE] != 0 { rs[RS_ALPHAFUNC] } else { 0 };
    PsBlock {
        stages,
        tfactor: color(rs[RS_TEXTUREFACTOR]),
        flags: [
            alpha_func,
            rs[RS_ALPHAREF] & 0xFF,
            (rs[RS_SHADEMODE] == 1) as u32,
            (fixed_function && rs[RS_SPECULARENABLE] != 0) as u32,
        ],
        c: s.ps_const,
        count: [count, 0, 0, 0],
        soft,
    }
}

pub fn as_bytes<T: Copy>(v: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) }
}

/// Byte equality without an early exit, which the compiler turns into wide compares instead of a
/// memcmp call. For the state blocks compared on nearly every draw.
pub fn bytes_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let tail = ca.remainder().iter().zip(cb.remainder()).fold(0u8, |d, (x, y)| d | (x ^ y)) as u64;
    let diff = ca.zip(cb).fold(tail, |d, (x, y)| d | (u64::from_ne_bytes(x.try_into().unwrap()) ^ u64::from_ne_bytes(y.try_into().unwrap())));
    diff == 0
}

pub fn same<T: Copy>(a: &T, b: &T) -> bool {
    bytes_equal(as_bytes(a), as_bytes(b))
}

#[cfg(test)]
mod tests {
    use super::bytes_equal;

    #[test]
    fn bytes_equal_finds_any_difference() {
        for len in [0usize, 1, 7, 8, 9, 64, 65, 1744] {
            let a: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            assert!(bytes_equal(&a, &a.clone()));
            for i in 0..len {
                let mut b = a.clone();
                b[i] ^= 0x10;
                assert!(!bytes_equal(&a, &b), "difference at {i} of {len} missed");
            }
        }
        assert!(!bytes_equal(&[1, 2], &[1, 2, 3]));
    }
}
