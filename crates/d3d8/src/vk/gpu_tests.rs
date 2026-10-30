//! Drives our Vulkan device through the D3D8 interface the way the game does, and checks the
//! pixels that come out.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW, WS_VISIBLE};

use crate::d3d::D3DPRESENT_PARAMETERS;

macro_rules! call {
    ($obj:expr, $slot:expr, fn($($t:ty),*) -> $r:ty $(, $arg:expr)*) => {{
        let o = $obj as *mut c_void;
        let f: unsafe extern "system" fn(*mut c_void $(, $t)*) -> $r =
            unsafe { std::mem::transmute(*(*(o as *const *const usize)).add($slot)) };
        unsafe { f(o $(, $arg)*) }
    }};
}

const CLEAR: usize = 36;
const BEGIN_SCENE: usize = 34;
const END_SCENE: usize = 35;
const PRESENT: usize = 15;
const GET_BACK_BUFFER: usize = 16;
const SET_RENDER_STATE: usize = 50;
const SET_TSS: usize = 63;
const SET_TEXTURE: usize = 61;
const SET_VERTEX_SHADER: usize = 76;
const DRAW_PRIMITIVE_UP: usize = 72;
const CREATE_TEXTURE: usize = 20;
const CREATE_PIXEL_SHADER: usize = 87;
const SET_PIXEL_SHADER: usize = 88;
const CREATE_VERTEX_SHADER: usize = 75;
const SET_VS_CONSTANT: usize = 79;
const CREATE_VERTEX_BUFFER: usize = 23;
const CREATE_INDEX_BUFFER: usize = 24;
const SET_STREAM_SOURCE: usize = 83;
const SET_INDICES: usize = 85;
const DRAW_INDEXED_PRIMITIVE: usize = 71;
const DRAW_PRIMITIVE: usize = 70;
const RESET: usize = 14;
const RELEASE: usize = 2;
const SET_RENDER_TARGET: usize = 31;
const GET_RENDER_TARGET: usize = 32;
const GET_DEPTH_STENCIL: usize = 33;
const CREATE_IMAGE_SURFACE: usize = 27;
const COPY_RECTS: usize = 28;
const SET_MATERIAL: usize = 42;
const SET_LIGHT: usize = 44;
const LIGHT_ENABLE: usize = 46;
const SET_TRANSFORM: usize = 37;
const SET_CLIP_PLANE: usize = 48;

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

const RS_ZENABLE: u32 = 7;
const RS_ZWRITEENABLE: u32 = 14;
const RS_ALPHATESTENABLE: u32 = 15;
const RS_SRCBLEND: u32 = 19;
const RS_DESTBLEND: u32 = 20;
const RS_CULLMODE: u32 = 22;
const RS_ZFUNC: u32 = 23;
const RS_ZBIAS: u32 = 47;
const RS_ALPHAREF: u32 = 24;
const RS_ALPHAFUNC: u32 = 25;
const RS_ALPHABLENDENABLE: u32 = 27;
const RS_STENCILENABLE: u32 = 52;
const RS_STENCILPASS: u32 = 55;
const RS_STENCILFUNC: u32 = 56;
const RS_STENCILREF: u32 = 57;
const RS_LIGHTING: u32 = 137;
const RS_CLIPPLANEENABLE: u32 = 152;
const RS_COLORWRITEENABLE: u32 = 168;

const FVF_RHW_DIFFUSE: u32 = 0x044;
const FVF_RHW_DIFFUSE_TEX1: u32 = 0x144;
const FVF_XYZ_DIFFUSE: u32 = 0x042;

fn window() -> HWND {
    let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
    let title: Vec<u16> = "cnc-vulkan test".encode_utf16().chain([0]).collect();
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            300,
            300,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    }
}

fn params(hwnd: HWND, w: u32, h: u32) -> D3DPRESENT_PARAMETERS {
    D3DPRESENT_PARAMETERS {
        BackBufferWidth: w,
        BackBufferHeight: h,
        BackBufferFormat: 22,
        BackBufferCount: 1,
        MultiSampleType: 0,
        SwapEffect: 1,
        hDeviceWindow: hwnd,
        Windowed: 1,
        EnableAutoDepthStencil: 1,
        AutoDepthStencilFormat: 75,
        Flags: 0,
        FullScreen_RefreshRateInHz: 0,
        FullScreen_PresentationInterval: 0,
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RhwVertex {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    color: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RhwTexVertex {
    x: f32,
    y: f32,
    z: f32,
    rhw: f32,
    color: u32,
    u: f32,
    v: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XyzVertex {
    x: f32,
    y: f32,
    z: f32,
    color: u32,
}

#[repr(C)]
struct LockedRect {
    pitch: i32,
    bits: *mut u8,
}

struct Dev(*mut c_void);

impl Dev {
    fn rs(&self, state: u32, value: u32) {
        assert_eq!(call!(self.0, SET_RENDER_STATE, fn(u32, u32) -> i32, state, value), 0);
    }

    fn tss(&self, stage: u32, ty: u32, value: u32) {
        assert_eq!(call!(self.0, SET_TSS, fn(u32, u32, u32) -> i32, stage, ty, value), 0);
    }

    fn clear(&self, flags: u32, color: u32, z: f32, stencil: u32) {
        assert_eq!(call!(self.0, CLEAR, fn(u32, *const c_void, u32, u32, f32, u32) -> i32, 0, std::ptr::null(), flags, color, z, stencil), 0);
    }

    fn fvf(&self, fvf: u32) {
        assert_eq!(call!(self.0, SET_VERTEX_SHADER, fn(u32) -> i32, fvf), 0);
    }

    fn quad_rhw(&self, x0: f32, y0: f32, x1: f32, y1: f32, z: f32, color: u32) {
        let v = [
            RhwVertex { x: x0, y: y0, z, rhw: 1.0, color },
            RhwVertex { x: x1, y: y0, z, rhw: 1.0, color },
            RhwVertex { x: x0, y: y1, z, rhw: 1.0, color },
            RhwVertex { x: x1, y: y1, z, rhw: 1.0, color },
        ];
        self.fvf(FVF_RHW_DIFFUSE);
        let hr = call!(self.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, v.as_ptr() as *const c_void, size_of::<RhwVertex>() as u32);
        assert_eq!(hr, 0);
    }

    fn quad_rhw_tex(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        let v = [
            RhwTexVertex { x: x0, y: y0, z: 0.0, rhw: 1.0, color, u: 0.0, v: 0.0 },
            RhwTexVertex { x: x1, y: y0, z: 0.0, rhw: 1.0, color, u: 1.0, v: 0.0 },
            RhwTexVertex { x: x0, y: y1, z: 0.0, rhw: 1.0, color, u: 0.0, v: 1.0 },
            RhwTexVertex { x: x1, y: y1, z: 0.0, rhw: 1.0, color, u: 1.0, v: 1.0 },
        ];
        self.fvf(FVF_RHW_DIFFUSE_TEX1);
        let hr = call!(self.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, v.as_ptr() as *const c_void, size_of::<RhwTexVertex>() as u32);
        assert_eq!(hr, 0);
    }

    fn texture(&self, w: u32, h: u32, format: u32, data: &[u8]) -> *mut c_void {
        let mut t = std::ptr::null_mut();
        let hr = call!(self.0, CREATE_TEXTURE, fn(u32, u32, u32, u32, u32, u32, *mut *mut c_void) -> i32, w, h, 1, 0, format, 1, &mut t);
        assert_eq!(hr, 0);
        let mut locked = LockedRect { pitch: 0, bits: std::ptr::null_mut() };
        assert_eq!(call!(t, 16, fn(u32, *mut LockedRect, *const c_void, u32) -> i32, 0, &mut locked, std::ptr::null(), 0), 0);
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), locked.bits, data.len()) };
        assert_eq!(call!(t, 17, fn(u32) -> i32, 0), 0);
        t
    }

    /// Reads the back buffer as A8R8G8B8 values.
    fn pixels(&self) -> (u32, Vec<u32>) {
        let mut surface = std::ptr::null_mut();
        assert_eq!(call!(self.0, GET_BACK_BUFFER, fn(u32, u32, *mut *mut c_void) -> i32, 0, 0, &mut surface), 0);
        let mut desc = [0u32; 8];
        assert_eq!(call!(surface, 8, fn(*mut u32) -> i32, desc.as_mut_ptr()), 0);
        let (w, h) = (desc[6], desc[7]);
        let mut locked = LockedRect { pitch: 0, bits: std::ptr::null_mut() };
        assert_eq!(call!(surface, 9, fn(*mut LockedRect, *const c_void, u32) -> i32, &mut locked, std::ptr::null(), 0x10), 0);
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let p = unsafe { locked.bits.add((y as i32 * locked.pitch) as usize + x as usize * 4) as *const u32 };
                out.push(unsafe { p.read_unaligned() });
            }
        }
        assert_eq!(call!(surface, 10, fn() -> i32), 0);
        call!(surface, RELEASE, fn() -> u32);
        (w, out)
    }

    fn pixel(&self, x: u32, y: u32) -> u32 {
        let (w, p) = self.pixels();
        p[(y * w + x) as usize] & 0xFF_FFFF
    }
}

fn close(a: u32, b: u32) -> bool {
    (0..3).all(|i| ((a >> (i * 8)) & 0xFF).abs_diff((b >> (i * 8)) & 0xFF) <= 3)
}

#[track_caller]
fn expect(d: &Dev, x: u32, y: u32, want: u32, what: &str) {
    let got = d.pixel(x, y);
    assert!(close(got, want), "{what}: pixel ({x}, {y}) is {got:06x}, expected {want:06x}");
}

#[test]
fn renders_through_vulkan() {
    let module = crate::loader::system_library("d3d8.dll");
    let create = unsafe { GetProcAddress(module, c"Direct3DCreate8".as_ptr() as *const u8) }.expect("Direct3DCreate8");
    let create: unsafe extern "system" fn(u32) -> *mut c_void = unsafe { std::mem::transmute(create) };
    let real = unsafe { create(220) };
    assert!(!real.is_null());
    let d3d = crate::direct3d::wrap(real);
    let hwnd = window();
    let mut pp = params(hwnd, 256, 256);
    let mut device = std::ptr::null_mut();
    let hr = call!(d3d, 15, fn(u32, u32, HWND, u32, *mut D3DPRESENT_PARAMETERS, *mut *mut c_void) -> i32, 0, 1, hwnd, 0x80, &mut pp, &mut device);
    assert_eq!(hr, 0, "CreateDevice");
    let d = Dev(device);

    call!(d.0, BEGIN_SCENE, fn() -> i32);
    d.clear(7, 0xFFFF_0000, 1.0, 0);
    expect(&d, 10, 10, 0xFF0000, "clear");

    d.rs(RS_LIGHTING, 0);
    d.rs(RS_CULLMODE, 1);
    d.rs(RS_ZENABLE, 0);
    d.quad_rhw(0.0, 0.0, 128.0, 128.0, 0.0, 0xFF00_FF00);
    expect(&d, 64, 64, 0x00FF00, "pretransformed quad");
    expect(&d, 127, 127, 0x00FF00, "pretransformed quad edge");
    expect(&d, 128, 128, 0xFF0000, "outside pretransformed quad");

    // D3D's y axis points up, the quad covers the top right quarter.
    let v = [
        XyzVertex { x: 0.0, y: 0.0, z: 0.5, color: 0xFF00_00FF },
        XyzVertex { x: 1.0, y: 0.0, z: 0.5, color: 0xFF00_00FF },
        XyzVertex { x: 0.0, y: 1.0, z: 0.5, color: 0xFF00_00FF },
        XyzVertex { x: 1.0, y: 1.0, z: 0.5, color: 0xFF00_00FF },
    ];
    d.fvf(FVF_XYZ_DIFFUSE);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, v.as_ptr() as *const c_void, size_of::<XyzVertex>() as u32);
    expect(&d, 192, 64, 0x0000FF, "transformed quad");
    expect(&d, 192, 192, 0xFF0000, "below transformed quad");

    // A user clip plane keeps world x <= 0.5. The view moves the world right by 0.25, so the quad
    // from x 0 to 1 lands at pixels 160 to 288 and is cut at 224.
    let shifted = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.25, 0.0, 0.0, 1.0f32];
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 2, shifted.as_ptr()), 0);
    let plane = [-1.0, 0.0, 0.0, 0.5f32];
    assert_eq!(call!(d.0, SET_CLIP_PLANE, fn(u32, *const f32) -> i32, 0, plane.as_ptr()), 0);
    d.rs(RS_CLIPPLANEENABLE, 1);
    let green = v.map(|p| XyzVertex { color: 0xFF00_FF00, ..p });
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, green.as_ptr() as *const c_void, size_of::<XyzVertex>() as u32);
    expect(&d, 200, 64, 0x00FF00, "clip plane, kept side");
    expect(&d, 240, 64, 0x0000FF, "clip plane, clipped side");
    expect(&d, 150, 64, 0x0000FF, "left of the moved quad");
    d.rs(RS_CLIPPLANEENABLE, 0);
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 2, IDENTITY.as_ptr()), 0);

    // A yellow texture modulated with white vertex color.
    let yellow: Vec<u8> = [0x00u8, 0xFF, 0xFF, 0xFF].repeat(16);
    let tex = d.texture(4, 4, 21, &yellow);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, tex);
    d.quad_rhw_tex(0.0, 192.0, 64.0, 256.0, 0xFFFF_FFFF);
    expect(&d, 32, 224, 0xFFFF00, "textured quad");

    // A DXT1 block of solid green.
    let block: [u8; 8] = [0xE0, 0x07, 0xE0, 0x07, 0, 0, 0, 0];
    let dxt = d.texture(4, 4, u32::from_le_bytes(*b"DXT1"), &block);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, dxt);
    d.quad_rhw_tex(64.0, 192.0, 128.0, 256.0, 0xFFFF_FFFF);
    expect(&d, 96, 224, 0x00FF00, "DXT1 quad");
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());

    // The game's "tex t1; mov r0, t1; mul r0, r0, v0" pixel shader.
    let ps_tokens = super::tests::shader_tokens(2);
    let mut ps = 0u32;
    assert_eq!(call!(d.0, CREATE_PIXEL_SHADER, fn(*const u32, *mut u32) -> i32, ps_tokens.as_ptr(), &mut ps), 0);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 1, tex);
    call!(d.0, SET_PIXEL_SHADER, fn(u32) -> i32, ps);
    d.quad_rhw_tex(128.0, 192.0, 192.0, 256.0, 0xFF80_8080);
    call!(d.0, SET_PIXEL_SHADER, fn(u32) -> i32, 0);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 1, std::ptr::null_mut::<c_void>());
    expect(&d, 160, 224, 0x808000, "pixel shader quad");

    // Alpha test rejects alpha 0x40 against a reference of 0x80.
    d.rs(RS_ALPHATESTENABLE, 1);
    d.rs(RS_ALPHAREF, 0x80);
    d.rs(RS_ALPHAFUNC, 7);
    d.quad_rhw(192.0, 192.0, 256.0, 256.0, 0.0, 0x40FF_FFFF);
    expect(&d, 224, 224, 0xFF0000, "alpha tested quad");
    d.rs(RS_ALPHATESTENABLE, 0);

    // Half transparent white over red.
    d.rs(RS_ALPHABLENDENABLE, 1);
    d.rs(RS_SRCBLEND, 5);
    d.rs(RS_DESTBLEND, 6);
    d.quad_rhw(192.0, 192.0, 256.0, 256.0, 0.0, 0x80FF_FFFF);
    expect(&d, 224, 224, 0xFF7F7F, "blended quad");
    d.rs(RS_ALPHABLENDENABLE, 0);

    // Depth: the nearer green quad wins over the red one drawn after it.
    d.clear(7, 0xFF00_0000, 1.0, 0);
    d.rs(RS_ZENABLE, 1);
    d.rs(RS_ZWRITEENABLE, 1);
    d.rs(RS_ZFUNC, 4);
    d.quad_rhw(0.0, 0.0, 64.0, 64.0, 0.3, 0xFF00_FF00);
    d.quad_rhw(32.0, 32.0, 96.0, 96.0, 0.6, 0xFFFF_0000);
    expect(&d, 40, 40, 0x00FF00, "depth tested overlap");
    expect(&d, 80, 80, 0xFF0000, "depth tested quad");

    // Perspective depth: the nearer quad (left half) wins in either draw order.
    let (near, far) = (1.0f32, 100.0f32);
    let q = far / (far - near);
    let proj: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, q, 1.0, 0.0, 0.0, -q * near, 0.0];
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 3, proj.as_ptr()), 0);
    let quad = |x0: f32, x1: f32, z: f32, color: u32| {
        let v = [
            XyzVertex { x: x0 * z, y: z, z, color },
            XyzVertex { x: x1 * z, y: z, z, color },
            XyzVertex { x: x0 * z, y: -z, z, color },
            XyzVertex { x: x1 * z, y: -z, z, color },
        ];
        d.fvf(FVF_XYZ_DIFFUSE);
        call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, v.as_ptr() as *const c_void, size_of::<XyzVertex>() as u32);
    };
    for near_first in [true, false] {
        d.clear(7, 0xFF00_0000, 1.0, 0);
        if near_first {
            quad(-1.0, 0.0, 5.0, 0xFF00_FF00);
            quad(-1.0, 1.0, 50.0, 0xFFFF_0000);
        } else {
            quad(-1.0, 1.0, 50.0, 0xFFFF_0000);
            quad(-1.0, 0.0, 5.0, 0xFF00_FF00);
        }
        expect(&d, 64, 128, 0x00FF00, "nearer projected quad");
        expect(&d, 192, 128, 0xFF0000, "farther projected quad");
    }

    // ZBIAS pulls a coplanar quad in front. Without it, LESS rejects the same depth.
    d.rs(RS_ZFUNC, 2);
    quad(-1.0, 1.0, 50.0, 0xFF00_00FF);
    expect(&d, 192, 128, 0xFF0000, "coplanar quad without bias");
    d.rs(RS_ZBIAS, 8);
    quad(-1.0, 1.0, 50.0, 0xFF00_FF00);
    expect(&d, 192, 128, 0x00FF00, "coplanar quad with bias");
    d.rs(RS_ZBIAS, 0);
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 3, IDENTITY.as_ptr()), 0);
    d.rs(RS_ZFUNC, 4);
    d.rs(RS_ZENABLE, 0);
    d.clear(7, 0xFF00_0000, 1.0, 0);

    // Stencil: mark a square without drawing color, then fill only where it is marked.
    d.rs(RS_STENCILENABLE, 1);
    d.rs(RS_STENCILFUNC, 8);
    d.rs(RS_STENCILPASS, 3);
    d.rs(RS_STENCILREF, 1);
    d.rs(RS_COLORWRITEENABLE, 0);
    d.quad_rhw(128.0, 0.0, 192.0, 64.0, 0.0, 0xFFFF_FFFF);
    d.rs(RS_COLORWRITEENABLE, 0xF);
    d.rs(RS_STENCILFUNC, 3);
    d.rs(RS_STENCILPASS, 1);
    d.quad_rhw(0.0, 0.0, 256.0, 256.0, 0.0, 0xFF00_00FF);
    d.rs(RS_STENCILENABLE, 0);
    expect(&d, 160, 32, 0x0000FF, "stencil marked area");
    expect(&d, 220, 32, 0x000000, "stencil unmarked area");

    // Vertex and index buffers.
    d.clear(1, 0xFF00_0000, 1.0, 0);
    let mut vb = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_VERTEX_BUFFER, fn(u32, u32, u32, u32, *mut *mut c_void) -> i32, 4 * 20, 8, FVF_RHW_DIFFUSE, 1, &mut vb), 0);
    let mut ib = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_INDEX_BUFFER, fn(u32, u32, u32, u32, *mut *mut c_void) -> i32, 12, 8, 101, 1, &mut ib), 0);
    let verts = [
        RhwVertex { x: 0.0, y: 0.0, z: 0.0, rhw: 1.0, color: 0xFFFF_FF00 },
        RhwVertex { x: 64.0, y: 0.0, z: 0.0, rhw: 1.0, color: 0xFFFF_FF00 },
        RhwVertex { x: 0.0, y: 64.0, z: 0.0, rhw: 1.0, color: 0xFFFF_FF00 },
        RhwVertex { x: 64.0, y: 64.0, z: 0.0, rhw: 1.0, color: 0xFFFF_FF00 },
    ];
    let mut p: *mut u8 = std::ptr::null_mut();
    assert_eq!(call!(vb, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, 0, 0, &mut p, 0), 0);
    unsafe { std::ptr::copy_nonoverlapping(verts.as_ptr() as *const u8, p, 80) };
    call!(vb, 12, fn() -> i32);
    // The game thread drops its copy after the first unlock, and a later lock reads it back.
    assert_eq!(call!(vb, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, 0, 0, &mut p, 0x10), 0);
    assert_eq!(unsafe { std::slice::from_raw_parts(p, 80) }, unsafe { std::slice::from_raw_parts(verts.as_ptr() as *const u8, 80) });
    call!(vb, 12, fn() -> i32);
    let indices: [u16; 6] = [0, 1, 2, 2, 1, 3];
    assert_eq!(call!(ib, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, 0, 0, &mut p, 0), 0);
    unsafe { std::ptr::copy_nonoverlapping(indices.as_ptr() as *const u8, p, 12) };
    call!(ib, 12, fn() -> i32);
    call!(d.0, SET_STREAM_SOURCE, fn(u32, *mut c_void, u32) -> i32, 0, vb, 20);
    call!(d.0, SET_INDICES, fn(*mut c_void, u32) -> i32, ib, 0);
    d.fvf(FVF_RHW_DIFFUSE);
    assert_eq!(call!(d.0, DRAW_INDEXED_PRIMITIVE, fn(u32, u32, u32, u32, u32) -> i32, 4, 0, 4, 0, 2), 0);
    expect(&d, 32, 32, 0xFFFF00, "indexed quad");

    // Relocking a buffer after a draw must not change what that draw already used.
    let moved: Vec<RhwVertex> = verts.iter().map(|v| RhwVertex { x: v.x + 64.0, color: 0xFF00_FFFF, ..*v }).collect();
    assert_eq!(call!(d.0, DRAW_INDEXED_PRIMITIVE, fn(u32, u32, u32, u32, u32) -> i32, 4, 0, 4, 0, 2), 0);
    assert_eq!(call!(vb, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, 0, 0, &mut p, 0), 0);
    unsafe { std::ptr::copy_nonoverlapping(moved.as_ptr() as *const u8, p, 80) };
    call!(vb, 12, fn() -> i32);
    assert_eq!(call!(d.0, DRAW_INDEXED_PRIMITIVE, fn(u32, u32, u32, u32, u32) -> i32, 4, 0, 4, 0, 2), 0);
    // A discard lock that rewrites only the color keeps the other draws intact too.
    assert_eq!(call!(vb, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, 0, 0, &mut p, 0x2000), 0);
    let recolored: Vec<RhwVertex> = moved.iter().map(|v| RhwVertex { y: v.y + 64.0, color: 0xFFFF_00FF, ..*v }).collect();
    unsafe { std::ptr::copy_nonoverlapping(recolored.as_ptr() as *const u8, p, 80) };
    call!(vb, 12, fn() -> i32);
    assert_eq!(call!(d.0, DRAW_INDEXED_PRIMITIVE, fn(u32, u32, u32, u32, u32) -> i32, 4, 0, 4, 0, 2), 0);
    expect(&d, 32, 32, 0xFFFF00, "draw before the relock");
    expect(&d, 96, 32, 0x00FFFF, "draw after the relock");
    expect(&d, 96, 96, 0xFF00FF, "draw after the discard lock");

    // A dynamic buffer is written straight into GPU memory. No kind of lock may change what an
    // earlier draw used, and a plain lock of one the GPU still reads keeps its contents.
    d.clear(1, 0xFF00_0000, 1.0, 0);
    let mut dvb = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_VERTEX_BUFFER, fn(u32, u32, u32, u32, *mut *mut c_void) -> i32, 8 * 20, 0x208, FVF_RHW_DIFFUSE, 0, &mut dvb), 0);
    let quad = |x: f32, y: f32, color: u32| {
        [(x, y), (x + 64.0, y), (x, y + 64.0), (x + 64.0, y + 64.0)].map(|(x, y)| RhwVertex { x, y, z: 0.0, rhw: 1.0, color })
    };
    let write = |flags: u32, offset: u32, q: [RhwVertex; 4]| {
        let mut p: *mut u8 = std::ptr::null_mut();
        assert_eq!(call!(dvb, 11, fn(u32, u32, *mut *mut u8, u32) -> i32, offset, 80, &mut p, flags), 0);
        unsafe { std::ptr::copy_nonoverlapping(q.as_ptr() as *const u8, p, 80) };
        call!(dvb, 12, fn() -> i32);
    };
    let draw = |start: u32| assert_eq!(call!(d.0, DRAW_PRIMITIVE, fn(u32, u32, u32) -> i32, 5, start, 2), 0);
    call!(d.0, SET_STREAM_SOURCE, fn(u32, *mut c_void, u32) -> i32, 0, dvb, 20);
    d.fvf(FVF_RHW_DIFFUSE);
    write(0x2000, 0, quad(0.0, 0.0, 0xFFFF_0000));
    draw(0);
    write(0x1000, 80, quad(64.0, 0.0, 0xFF00_FF00));
    draw(4);
    write(0x2000, 0, quad(0.0, 64.0, 0xFF00_00FF));
    draw(0);
    write(0, 80, quad(64.0, 64.0, 0xFFFF_FFFF));
    draw(4);
    expect(&d, 32, 32, 0xFF0000, "dynamic buffer before its locks");
    expect(&d, 96, 32, 0x00FF00, "dynamic buffer no-overwrite lock");
    expect(&d, 32, 96, 0x0000FF, "dynamic buffer discard lock");
    expect(&d, 96, 96, 0xFFFFFF, "dynamic buffer plain lock");
    d.clear(1, 0xFF00_0000, 1.0, 0);
    draw(0);
    draw(4);
    expect(&d, 32, 96, 0x0000FF, "dynamic buffer kept through a plain lock");
    expect(&d, 96, 96, 0xFFFFFF, "dynamic buffer written by a plain lock");
    expect(&d, 32, 32, 0x000000, "dynamic buffer old quad gone");
    call!(d.0, SET_STREAM_SOURCE, fn(u32, *mut c_void, u32) -> i32, 0, std::ptr::null_mut::<c_void>(), 0);
    call!(dvb, RELEASE, fn() -> u32);

    // The tree sway vertex shader, with constants that make it pass positions through.
    let (decl, function) = super::tests::tree_shader();
    let mut vs = 0u32;
    assert_eq!(call!(d.0, CREATE_VERTEX_SHADER, fn(*const u32, *const u32, *mut u32, u32) -> i32, decl.as_ptr(), function.as_ptr(), &mut vs, 0), 0);
    let mut constants = [[0.0f32; 4]; 96];
    constants[4] = [1.0, 0.0, 0.0, 0.0];
    constants[5] = [0.0, 1.0, 0.0, 0.0];
    constants[6] = [0.0, 0.0, 1.0, 0.0];
    constants[7] = [0.0, 0.0, 0.0, 1.0];
    call!(d.0, SET_VS_CONSTANT, fn(u32, *const [f32; 4], u32) -> i32, 0, constants.as_ptr(), 96);
    #[repr(C)]
    struct TreeVertex {
        pos: [f32; 3],
        sway: [f32; 3],
        color: u32,
        uv: [f32; 2],
    }
    let tree = [
        TreeVertex { pos: [-1.0, -1.0, 0.5], sway: [0.0, 1.0, 0.0], color: 0xFF00_FFFF, uv: [0.0, 0.0] },
        TreeVertex { pos: [0.0, -1.0, 0.5], sway: [0.0, 1.0, 0.0], color: 0xFF00_FFFF, uv: [0.0, 0.0] },
        TreeVertex { pos: [-1.0, 0.0, 0.5], sway: [0.0, 1.0, 0.0], color: 0xFF00_FFFF, uv: [0.0, 0.0] },
        TreeVertex { pos: [0.0, 0.0, 0.5], sway: [0.0, 1.0, 0.0], color: 0xFF00_FFFF, uv: [0.0, 0.0] },
    ];
    call!(d.0, SET_VERTEX_SHADER, fn(u32) -> i32, vs);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, tree.as_ptr() as *const c_void, size_of::<TreeVertex>() as u32);
    expect(&d, 64, 192, 0x00FFFF, "tree vertex shader quad");

    // Render to a texture without a depth buffer, then draw with it.
    let mut back = std::ptr::null_mut();
    let mut depth = std::ptr::null_mut();
    call!(d.0, GET_RENDER_TARGET, fn(*mut *mut c_void) -> i32, &mut back);
    call!(d.0, GET_DEPTH_STENCIL, fn(*mut *mut c_void) -> i32, &mut depth);
    let mut rt_tex = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_TEXTURE, fn(u32, u32, u32, u32, u32, u32, *mut *mut c_void) -> i32, 64, 64, 1, 1, 21, 0, &mut rt_tex), 0);
    let mut rt_surface = std::ptr::null_mut();
    assert_eq!(call!(rt_tex, 15, fn(u32, *mut *mut c_void) -> i32, 0, &mut rt_surface), 0);
    assert_eq!(call!(d.0, SET_RENDER_TARGET, fn(*mut c_void, *mut c_void) -> i32, rt_surface, std::ptr::null_mut::<c_void>()), 0);
    d.clear(1, 0xFFFF_00FF, 1.0, 0);
    assert_eq!(call!(d.0, SET_RENDER_TARGET, fn(*mut c_void, *mut c_void) -> i32, back, depth), 0);
    d.fvf(FVF_RHW_DIFFUSE_TEX1);
    call!(d.0, SET_VERTEX_SHADER, fn(u32) -> i32, FVF_RHW_DIFFUSE_TEX1);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, rt_tex);
    d.quad_rhw_tex(192.0, 0.0, 256.0, 64.0, 0xFFFF_FFFF);
    expect(&d, 224, 32, 0xFF00FF, "render target texture");
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    call!(rt_surface, RELEASE, fn() -> u32);
    call!(rt_tex, RELEASE, fn() -> u32);
    call!(back, RELEASE, fn() -> u32);
    call!(depth, RELEASE, fn() -> u32);

    // An image surface copied into a texture level.
    let mut image = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_IMAGE_SURFACE, fn(u32, u32, u32, *mut *mut c_void) -> i32, 4, 4, 21, &mut image), 0);
    let mut locked = LockedRect { pitch: 0, bits: std::ptr::null_mut() };
    assert_eq!(call!(image, 9, fn(*mut LockedRect, *const c_void, u32) -> i32, &mut locked, std::ptr::null(), 0), 0);
    for i in 0..16 {
        unsafe { (locked.bits as *mut u32).add(i).write_unaligned(0xFF00_FFFF) };
    }
    call!(image, 10, fn() -> i32);
    let copy_tex = d.texture(4, 4, 21, &[0u8; 64]);
    let mut level = std::ptr::null_mut();
    call!(copy_tex, 15, fn(u32, *mut *mut c_void) -> i32, 0, &mut level);
    assert_eq!(call!(d.0, COPY_RECTS, fn(*mut c_void, *const c_void, u32, *mut c_void, *const c_void) -> i32, image, std::ptr::null(), 0, level, std::ptr::null()), 0);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, copy_tex);
    d.quad_rhw_tex(192.0, 64.0, 256.0, 128.0, 0xFFFF_FFFF);
    expect(&d, 224, 96, 0x00FFFF, "copied image surface");
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    call!(level, RELEASE, fn() -> u32);
    call!(copy_tex, RELEASE, fn() -> u32);
    call!(image, RELEASE, fn() -> u32);

    // A lit quad: a directional light straight into its normal, half grey material.
    #[repr(C)]
    struct LitVertex {
        pos: [f32; 3],
        normal: [f32; 3],
    }
    let lit = [
        LitVertex { pos: [-1.0, 0.0, 0.5], normal: [0.0, 0.0, -1.0] },
        LitVertex { pos: [0.0, 0.0, 0.5], normal: [0.0, 0.0, -1.0] },
        LitVertex { pos: [-1.0, -1.0, 0.5], normal: [0.0, 0.0, -1.0] },
        LitVertex { pos: [0.0, -1.0, 0.5], normal: [0.0, 0.0, -1.0] },
    ];
    let mut light = [0u32; 26];
    light[0] = 3;
    light[1] = 1.0f32.to_bits();
    light[2] = 1.0f32.to_bits();
    light[3] = 1.0f32.to_bits();
    light[18] = 1.0f32.to_bits();
    let mut material = [0f32; 17];
    material[0] = 0.5;
    material[1] = 0.5;
    material[2] = 0.5;
    material[3] = 1.0;
    call!(d.0, SET_LIGHT, fn(u32, *const u32) -> i32, 0, light.as_ptr());
    call!(d.0, LIGHT_ENABLE, fn(u32, i32) -> i32, 0, 1);
    call!(d.0, SET_MATERIAL, fn(*const f32) -> i32, material.as_ptr());
    d.rs(RS_LIGHTING, 1);
    d.fvf(0x012);
    d.tss(0, 1, 3);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, lit.as_ptr() as *const c_void, size_of::<LitVertex>() as u32);
    expect(&d, 64, 160, 0x808080, "lit quad");
    d.rs(RS_LIGHTING, 0);
    d.tss(0, 1, 4);
    call!(d.0, END_SCENE, fn() -> i32);

    for _ in 0..3 {
        call!(d.0, BEGIN_SCENE, fn() -> i32);
        d.clear(7, 0xFF20_4060, 1.0, 0);
        call!(d.0, END_SCENE, fn() -> i32);
        assert_eq!(call!(d.0, PRESENT, fn(*const c_void, *const c_void, HWND, *const c_void) -> i32, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null()), 0);
    }

    // The present pass's shadow brightness lift and calibration pattern, from black on the left and
    // white on the right.
    let mut back = std::ptr::null_mut();
    let mut depth = std::ptr::null_mut();
    call!(d.0, GET_RENDER_TARGET, fn(*mut *mut c_void) -> i32, &mut back);
    call!(d.0, GET_DEPTH_STENCIL, fn(*mut *mut c_void) -> i32, &mut depth);
    let mut picture = std::ptr::null_mut();
    assert_eq!(call!(d.0, CREATE_TEXTURE, fn(u32, u32, u32, u32, u32, u32, *mut *mut c_void) -> i32, 256, 256, 1, 1, 21, 0, &mut picture), 0);
    let mut picture_surface = std::ptr::null_mut();
    assert_eq!(call!(picture, 15, fn(u32, *mut *mut c_void) -> i32, 0, &mut picture_surface), 0);
    assert_eq!(call!(d.0, SET_RENDER_TARGET, fn(*mut c_void, *mut c_void) -> i32, picture_surface, std::ptr::null_mut::<c_void>()), 0);
    d.clear(1, 0xFF00_0000, 1.0, 0);
    d.quad_rhw(128.0, 0.0, 256.0, 256.0, 0.0, 0xFFFF_FFFF);
    assert_eq!(call!(d.0, SET_RENDER_TARGET, fn(*mut c_void, *mut c_void) -> i32, back, depth), 0);
    let device = d.0 as *mut crate::vk::device::Device;
    crate::vk::device::present_pass_for_test(device, picture, 0.04, None);
    expect(&d, 64, 128, 0x0A0A0A, "black lifted by 4%");
    expect(&d, 192, 128, 0xFFFFFF, "white with a lift");
    // The largest pattern in 4..250 by 10..58 has unit 6 at (4, 16): the box covers 4..250 by
    // 16..52, the squares y 22..46 from x 10, 30 apart.
    let pattern = crate::vk::post::calibration_pattern([4.0, 10.0, 246.0, 48.0]);
    assert_eq!(pattern, [4.0, 16.0, 6.0, 1.0], "calibration pattern placement");
    crate::vk::device::present_pass_for_test(device, picture, 0.0, Some(pattern));
    let level = |x: u32, y: u32| d.pixel(x, y) & 0xFF;
    assert_eq!(level(8, 20), 0, "calibration box");
    assert_eq!(level(4, 30), 64, "calibration box outline");
    assert_eq!(level(20, 34), 1, "faintest calibration square");
    assert_eq!(level(230, 34), 16, "brightest calibration square");
    assert_eq!(level(200, 100), 255, "picture beside the pattern");
    call!(picture_surface, RELEASE, fn() -> u32);
    call!(picture, RELEASE, fn() -> u32);
    call!(back, RELEASE, fn() -> u32);
    call!(depth, RELEASE, fn() -> u32);

    // The 3D world at half size in its own target, then interface drawn over it at full size.
    crate::vk::device::set_render_scale(50);
    d.clear(7, 0xFF00_0000, 1.0, 0);
    crate::scene::set_in_scene(true);
    d.clear(1, 0xFF00_00FF, 1.0, 0);
    d.quad_rhw(0.0, 0.0, 128.0, 256.0, 0.0, 0xFF00_FF00);
    crate::scene::set_in_scene(false);
    d.quad_rhw(200.0, 10.0, 203.0, 13.0, 0.0, 0xFFFF_0000);
    expect(&d, 64, 128, 0x00FF00, "world drawn at half size");
    expect(&d, 192, 128, 0x0000FF, "world cleared at half size");
    expect(&d, 200, 10, 0xFF0000, "interface over the world");
    expect(&d, 202, 12, 0xFF0000, "interface over the world, last pixel");
    expect(&d, 203, 12, 0x0000FF, "interface edge stays sharp");
    crate::vk::device::set_render_scale(100);

    // Sharpening at full size leaves flat areas as they are.
    crate::vk::device::set_sharpness(100);
    crate::scene::set_in_scene(true);
    d.clear(1, 0xFF40_8020, 1.0, 0);
    d.quad_rhw(0.0, 0.0, 128.0, 256.0, 0.0, 0xFF20_4080);
    crate::scene::set_in_scene(false);
    d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
    expect(&d, 64, 128, 0x204080, "sharpened world, flat area");
    expect(&d, 192, 128, 0x408020, "sharpened world, other flat area");
    crate::vk::device::set_sharpness(0);

    // 4x multisampling smooths a diagonal edge in the world.
    crate::vk::device::set_msaa(4);
    d.clear(1, 0xFF00_00FF, 1.0, 0);
    crate::scene::set_in_scene(true);
    d.clear(1, 0xFF00_00FF, 1.0, 0);
    let tri = [
        RhwVertex { x: 0.0, y: 0.0, z: 0.0, rhw: 1.0, color: 0xFF00_FF00 },
        RhwVertex { x: 256.0, y: 0.0, z: 0.0, rhw: 1.0, color: 0xFF00_FF00 },
        RhwVertex { x: 256.0, y: 256.0, z: 0.0, rhw: 1.0, color: 0xFF00_FF00 },
    ];
    d.fvf(FVF_RHW_DIFFUSE);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 4, 1, tri.as_ptr() as *const c_void, size_of::<RhwVertex>() as u32);
    crate::scene::set_in_scene(false);
    d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
    expect(&d, 200, 20, 0x00FF00, "multisampled world, inside");
    expect(&d, 20, 200, 0x0000FF, "multisampled world, outside");
    let edge = d.pixel(100, 100);
    let (g, b) = ((edge >> 8) & 0xFF, edge & 0xFF);
    assert!(g > 0x20 && g < 0xE0 && b > 0x20 && b < 0xE0, "multisampled edge pixel is {edge:06x}, expected a blend");

    // Sorted translucent polygons go into the single sampled world, still depth tested against
    // what was drawn before, and the multisampled edge drawn before stays smooth.
    d.rs(RS_ZENABLE, 1);
    d.rs(RS_ZWRITEENABLE, 1);
    d.rs(RS_ZFUNC, 4);
    crate::scene::set_in_scene(true);
    d.clear(7, 0xFF00_00FF, 1.0, 0);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 4, 1, tri.as_ptr() as *const c_void, size_of::<RhwVertex>() as u32);
    d.quad_rhw(0.0, 0.0, 128.0, 256.0, 0.5, 0xFF00_FF00);
    crate::scene::mark_opaque_done();
    d.rs(RS_ZWRITEENABLE, 0);
    d.rs(RS_ALPHABLENDENABLE, 1);
    d.rs(RS_SRCBLEND, 5);
    d.rs(RS_DESTBLEND, 6);
    crate::scene::set_in_sorted(true);
    d.quad_rhw(0.0, 200.0, 256.0, 216.0, 0.8, 0x80FF_0000);
    crate::scene::set_in_sorted(false);
    crate::scene::set_in_scene(false);
    d.rs(RS_ALPHABLENDENABLE, 0);
    d.rs(RS_ZWRITEENABLE, 1);
    d.rs(RS_ZENABLE, 0);
    d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
    expect(&d, 64, 208, 0x00FF00, "sorted polygon behind the world stays hidden");
    expect(&d, 140, 240, 0x0000FF, "single sampled world keeps the clear");
    let over = d.pixel(140, 208);
    assert!((over >> 16) & 0xFF > 0x70 && over & 0xFF > 0x70 && (over >> 8) & 0xFF < 0x10, "sorted polygon over the clear is {over:06x}, expected a blend of red and blue");
    let edge = d.pixel(180, 180);
    let (g, b) = ((edge >> 8) & 0xFF, edge & 0xFF);
    assert!(g > 0x20 && g < 0xE0 && b > 0x20 && b < 0xE0, "multisampled edge after the switch is {edge:06x}, expected a blend");
    crate::vk::device::set_msaa(1);

    // FXAA blends along the same edge.
    crate::vk::device::set_fxaa(true);
    crate::scene::set_in_scene(true);
    d.clear(1, 0xFF00_00FF, 1.0, 0);
    call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 4, 1, tri.as_ptr() as *const c_void, size_of::<RhwVertex>() as u32);
    crate::scene::set_in_scene(false);
    d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
    expect(&d, 200, 20, 0x00FF00, "FXAA world, inside");
    let blended = (95..105).any(|x| {
        let p = d.pixel(x, 100);
        let (g, b) = ((p >> 8) & 0xFF, p & 0xFF);
        g > 0x20 && g < 0xE0 && b > 0x20 && b < 0xE0
    });
    assert!(blended, "FXAA left the edge at row 100 unblended");
    crate::vk::device::set_fxaa(false);

    // Ambient occlusion darkens a floor where it meets a wall, and leaves it alone far away.
    let (near, far) = (1.0f32, 1000.0f32);
    let q = far / (far - near);
    let perspective: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, q, 1.0, 0.0, 0.0, -q * near, 0.0];
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 3, perspective.as_ptr()), 0);
    let quad3 = |v: [[f32; 3]; 4], color: u32| {
        let v = v.map(|p| XyzVertex { x: p[0], y: p[1], z: p[2], color });
        d.fvf(FVF_XYZ_DIFFUSE);
        call!(d.0, DRAW_PRIMITIVE_UP, fn(u32, u32, *const c_void, u32) -> i32, 5, 2, v.as_ptr() as *const c_void, size_of::<XyzVertex>() as u32);
    };
    crate::vk::device::set_ambient_occlusion(2);
    crate::vk::device::set_soft_particles(false);
    d.rs(RS_ZENABLE, 1);
    d.rs(RS_ZWRITEENABLE, 1);
    d.rs(RS_ZFUNC, 4);
    for samples in [1, 4] {
        crate::vk::device::set_msaa(samples);
        crate::scene::set_in_scene(true);
        d.clear(7, 0xFF00_0000, 1.0, 0);
        quad3([[-60.0, -10.0, 100.0], [60.0, -10.0, 100.0], [-60.0, -10.0, 5.0], [60.0, -10.0, 5.0]], 0xFFFF_FFFF);
        quad3([[-60.0, 60.0, 100.0], [60.0, 60.0, 100.0], [-60.0, -10.0, 100.0], [60.0, -10.0, 100.0]], 0xFFFF_FFFF);
        crate::scene::mark_opaque_done();
        crate::scene::set_in_scene(false);
        d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
        let (w, px) = d.pixels();
        let contact = (128..150).map(|y| px[(y * w + 128) as usize] & 0xFF).min().unwrap();
        let open = px[(240 * w + 128) as usize] & 0xFF;
        assert!(contact + 20 < open, "ambient occlusion with {samples} samples: floor at the wall is {contact:#x}, far from it {open:#x}");
    }
    crate::vk::device::set_msaa(1);

    // The occlusion moves with the wall as it slides down by about a quarter pixel per frame, near
    // and far from the camera, and doesn't trail behind it after a jump. Returns where the occlusion
    // sits relative to the edge of the wall, in pixels.
    let wall_frame = |z: f32, dy: f32| -> f32 {
        let (bottom, top) = (-10.0 - dy, 60.0 - dy);
        crate::scene::set_in_scene(true);
        d.clear(7, 0xFF00_0000, 1.0, 0);
        quad3([[-60.0, bottom, z], [60.0, bottom, z], [-60.0, bottom, 5.0], [60.0, bottom, 5.0]], 0xFFFF_FFFF);
        quad3([[-60.0, top, z], [60.0, top, z], [-60.0, bottom, z], [60.0, bottom, z]], 0xFFFF_FFFF);
        crate::scene::mark_opaque_done();
        crate::scene::set_in_scene(false);
        d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
        let edge = 128.0 - bottom / z * 128.0;
        let (w, px) = d.pixels();
        assert_eq!(call!(d.0, PRESENT, fn(*const c_void, *const c_void, HWND, *const c_void) -> i32, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null()), 0);
        // Seen from here the occlusion lies mostly on the wall, just above the edge.
        let reach = (16.0 * 128.0 / z) as u32;
        let (mut sum, mut weight) = (0.0, 0.0);
        for y in edge as u32 - reach..(edge as u32 + 6).min(255) {
            let dark = 255.0 - (px[(y * w + 128) as usize] & 0xFF) as f32;
            sum += dark * (y as f32 + 0.5);
            weight += dark;
        }
        assert!(weight > 0.0, "no occlusion at the wall at depth {z}");
        sum / weight - edge
    };
    let mut unsteady = Vec::new();
    for z in [100.0f32, 30.0] {
        let offsets: Vec<f32> = (0..16).map(|k| wall_frame(z, k as f32 * 0.27 * z / 100.0)).collect();
        let spread = offsets.iter().cloned().fold(f32::MIN, f32::max) - offsets.iter().cloned().fold(f32::MAX, f32::min);
        // The wall's own edge steps a whole pixel at a time.
        if spread >= 1.0 {
            unsteady.push(format!("at depth {z} it sits {offsets:?} pixels from the edge"));
        }
    }
    assert!(unsteady.is_empty(), "the occlusion at a wall moving smoothly wanders: {}", unsteady.join(", "));
    wall_frame(100.0, 9.0);
    let steady = wall_frame(100.0, 9.0);
    wall_frame(100.0, 0.0);
    let jumped = wall_frame(100.0, 9.0);
    assert!((steady - jumped).abs() < 0.3, "after a jump the occlusion sits {jumped} pixels from the edge, {steady} when still");
    crate::vk::device::set_ambient_occlusion(0);

    // Soft particles fade where they nearly touch the wall behind them. A right-handed camera
    // like WW3D's: it looks down -z.
    let right_handed: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, -q, -1.0, 0.0, 0.0, -q * near, 0.0];
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 3, right_handed.as_ptr()), 0);
    crate::vk::device::set_soft_particles(true);
    crate::scene::set_in_scene(true);
    d.clear(7, 0xFF00_0000, 1.0, 0);
    quad3([[-100.0, 100.0, -50.0], [100.0, 100.0, -50.0], [-100.0, -100.0, -50.0], [100.0, -100.0, -50.0]], 0xFF00_00FF);
    crate::scene::mark_opaque_done();
    d.rs(RS_ZWRITEENABLE, 0);
    d.rs(RS_ALPHABLENDENABLE, 1);
    d.rs(RS_SRCBLEND, 5);
    d.rs(RS_DESTBLEND, 6);
    crate::scene::set_in_sorted(true);
    quad3([[-48.0, 40.0, -48.0], [0.0, 40.0, -48.0], [-48.0, -40.0, -48.0], [0.0, -40.0, -48.0]], 0xFFFF_0000);
    quad3([[0.0, 8.0, -20.0], [20.0, 8.0, -20.0], [0.0, -8.0, -20.0], [20.0, -8.0, -20.0]], 0xFFFF_0000);
    // A polygon slanted away from the camera, like a line or marker lying on the ground, isn't faded.
    quad3([[10.0, -30.0, -45.0], [40.0, -30.0, -49.5], [10.0, -40.0, -45.0], [40.0, -40.0, -49.5]], 0xFFFF_0000);
    crate::scene::set_in_sorted(false);
    crate::scene::set_in_scene(false);
    d.rs(RS_ALPHABLENDENABLE, 0);
    d.quad_rhw(250.0, 250.0, 251.0, 251.0, 0.0, 0xFFFF_0000);
    expect(&d, 64, 128, 0x4000BF, "particle 2 units in front of the wall, a quarter visible");
    expect(&d, 192, 128, 0xFF0000, "particle 30 units in front of the wall, fully visible");
    expect(&d, 220, 222, 0xFF0000, "slanted polygon a unit in front of the wall, fully visible");
    crate::vk::device::set_soft_particles(false);
    d.rs(RS_ZWRITEENABLE, 1);
    d.rs(RS_ZENABLE, 0);
    assert_eq!(call!(d.0, SET_TRANSFORM, fn(u32, *const f32) -> i32, 3, IDENTITY.as_ptr()), 0);

    call!(tex, RELEASE, fn() -> u32);
    call!(dxt, RELEASE, fn() -> u32);
    call!(d.0, SET_STREAM_SOURCE, fn(u32, *mut c_void, u32) -> i32, 0, std::ptr::null_mut::<c_void>(), 0);
    call!(d.0, SET_INDICES, fn(*mut c_void, u32) -> i32, std::ptr::null_mut::<c_void>(), 0);
    call!(vb, RELEASE, fn() -> u32);
    call!(ib, RELEASE, fn() -> u32);

    // A released texture's image is used again for the next texture of its kind, with the new
    // texture's pixels.
    let first = d.texture(4, 4, 21, &[0x00u8, 0xFF, 0xFF, 0xFF].repeat(16));
    // Locked again, a texture level reads back what the first lock wrote.
    let mut locked = LockedRect { pitch: 0, bits: std::ptr::null_mut() };
    assert_eq!(call!(first, 16, fn(u32, *mut LockedRect, *const c_void, u32) -> i32, 0, &mut locked, std::ptr::null(), 0x10), 0);
    assert_eq!(unsafe { std::slice::from_raw_parts(locked.bits as *const u8, 64) }, [0x00u8, 0xFF, 0xFF, 0xFF].repeat(16).as_slice());
    assert_eq!(call!(first, 17, fn(u32) -> i32, 0), 0);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, first);
    d.quad_rhw_tex(0.0, 0.0, 16.0, 16.0, 0xFFFF_FFFF);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    call!(first, RELEASE, fn() -> u32);
    for _ in 0..3 {
        assert_eq!(call!(d.0, PRESENT, fn(*const c_void, *const c_void, HWND, *const c_void) -> i32, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null()), 0);
    }
    let second = d.texture(4, 4, 21, &[0xFFu8, 0x00, 0x00, 0xFF].repeat(16));
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, second);
    d.quad_rhw_tex(0.0, 0.0, 16.0, 16.0, 0xFFFF_FFFF);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    expect(&d, 8, 8, 0x0000FF, "texture on a recycled image");
    call!(second, RELEASE, fn() -> u32);

    // A texture marked smooth (the fog of war) is drawn from a larger copy through a cubic
    // B-spline, so a hard edge between two texels comes out as a gradient.
    let edge: Vec<u8> = (0..16).flat_map(|i| if i % 4 < 2 { [0u8, 0, 0, 0xFF] } else { [0xFF, 0xFF, 0xFF, 0xFF] }).collect();
    let fog = d.texture(4, 4, 21, &edge);
    let data = unsafe { &(*super::texture::texture_from_ptr(fog).unwrap()).data };
    data.smooth.set(true);
    d.clear(1, 0xFF00_0000, 1.0, 0);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, fog);
    d.quad_rhw_tex(0.0, 0.0, 64.0, 16.0, 0xFFFF_FFFF);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    expect(&d, 2, 8, 0x000000, "smoothed texture, dark side");
    expect(&d, 62, 8, 0xFFFFFF, "smoothed texture, bright side");
    let middle = d.pixel(31, 8) & 0xFF;
    assert!((40..216).contains(&middle), "smoothed texture: the edge at pixel 31 is {middle}, expected a value between");

    // The game writes the fog right before a Clear that uploads every bound texture. The smoothed
    // copy still has to follow the write.
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, fog);
    let white = [0xFFu8; 64];
    let mut locked = LockedRect { pitch: 0, bits: std::ptr::null_mut() };
    assert_eq!(call!(fog, 16, fn(u32, *mut LockedRect, *const c_void, u32) -> i32, 0, &mut locked, std::ptr::null(), 0), 0);
    unsafe { std::ptr::copy_nonoverlapping(white.as_ptr(), locked.bits, white.len()) };
    assert_eq!(call!(fog, 17, fn(u32) -> i32, 0), 0);
    d.clear(1, 0xFF00_0000, 1.0, 0);
    d.quad_rhw_tex(0.0, 0.0, 64.0, 16.0, 0xFFFF_FFFF);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    expect(&d, 2, 8, 0xFFFFFF, "smoothed texture written before a clear, formerly dark side");
    call!(fog, RELEASE, fn() -> u32);

    // Alpha is smoothed too: the radar's fog is black with alpha, blended over the map.
    let edge: Vec<u8> = (0..16).flat_map(|i| [0u8, 0, 0, if i % 4 < 2 { 0 } else { 0xFF }]).collect();
    let fog = d.texture(4, 4, 21, &edge);
    let data = unsafe { &(*super::texture::texture_from_ptr(fog).unwrap()).data };
    data.smooth.set(true);
    d.clear(1, 0xFFFF_FFFF, 1.0, 0);
    d.rs(RS_ALPHABLENDENABLE, 1);
    d.rs(RS_SRCBLEND, 5);
    d.rs(RS_DESTBLEND, 6);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, fog);
    d.quad_rhw_tex(0.0, 0.0, 64.0, 16.0, 0xFFFF_FFFF);
    call!(d.0, SET_TEXTURE, fn(u32, *mut c_void) -> i32, 0, std::ptr::null_mut::<c_void>());
    d.rs(RS_ALPHABLENDENABLE, 0);
    expect(&d, 2, 8, 0xFFFFFF, "smoothed alpha, clear side");
    expect(&d, 62, 8, 0x000000, "smoothed alpha, shrouded side");
    let middle = d.pixel(31, 8) & 0xFF;
    assert!((40..216).contains(&middle), "smoothed alpha: the edge at pixel 31 is {middle}, expected a value between");
    call!(fog, RELEASE, fn() -> u32);

    let mut pp = params(hwnd, 128, 96);
    assert_eq!(call!(d.0, RESET, fn(*mut D3DPRESENT_PARAMETERS) -> i32, &mut pp), 0);
    d.clear(7, 0xFF11_2233, 1.0, 0);
    let (w, pixels) = d.pixels();
    assert_eq!((w, pixels.len()), (128, 128 * 96));
    assert!(close(pixels[0] & 0xFF_FFFF, 0x112233), "clear after reset");
    assert_eq!(call!(d.0, PRESENT, fn(*const c_void, *const c_void, HWND, *const c_void) -> i32, std::ptr::null(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null()), 0);

    call!(d.0, RELEASE, fn() -> u32);
    unsafe { DestroyWindow(hwnd) };
    assert_eq!(super::validation::errors(), 0, "vulkan validation errors, see the messages above");
}
