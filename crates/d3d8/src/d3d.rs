#![allow(non_snake_case, clippy::upper_case_acronyms)]

use windows_sys::Win32::Foundation::HWND;

pub type HRESULT = i32;

pub const D3D_OK: HRESULT = 0;

pub const D3DPRESENT_INTERVAL_ONE: u32 = 1;
pub const D3DPRESENT_INTERVAL_IMMEDIATE: u32 = 0x8000_0000;

pub const D3DSWAPEFFECT_DISCARD: u32 = 1;
pub const D3DSWAPEFFECT_COPY_VSYNC: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct D3DPRESENT_PARAMETERS {
    pub BackBufferWidth: u32,
    pub BackBufferHeight: u32,
    pub BackBufferFormat: u32,
    pub BackBufferCount: u32,
    pub MultiSampleType: u32,
    pub SwapEffect: u32,
    pub hDeviceWindow: HWND,
    pub Windowed: i32,
    pub EnableAutoDepthStencil: i32,
    pub AutoDepthStencilFormat: u32,
    pub Flags: u32,
    pub FullScreen_RefreshRateInHz: u32,
    pub FullScreen_PresentationInterval: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct D3DDISPLAYMODE {
    pub Width: u32,
    pub Height: u32,
    pub RefreshRate: u32,
    pub Format: u32,
}

#[repr(C)]
pub struct D3DADAPTER_IDENTIFIER8 {
    pub Driver: [u8; 512],
    pub Description: [u8; 512],
    pub DriverVersion: u64,
    pub VendorId: u32,
    pub DeviceId: u32,
    pub SubSysId: u32,
    pub Revision: u32,
    pub DeviceIdentifier: [u8; 16],
    pub WHQLLevel: u32,
}

pub fn format_name(format: u32) -> String {
    let name = match format {
        0 => "UNKNOWN",
        20 => "R8G8B8",
        21 => "A8R8G8B8",
        22 => "X8R8G8B8",
        23 => "R5G6B5",
        24 => "X1R5G5B5",
        25 => "A1R5G5B5",
        26 => "A4R4G4B4",
        27 => "R3G3B2",
        28 => "A8",
        29 => "A8R3G3B2",
        30 => "X4R4G4B4",
        40 => "A8P8",
        41 => "P8",
        50 => "L8",
        51 => "A8L8",
        52 => "A4L4",
        60 => "V8U8",
        61 => "L6V5U5",
        62 => "X8L8V8U8",
        63 => "Q8W8V8U8",
        64 => "V16U16",
        65 => "W11V11U10",
        70 => "D16_LOCKABLE",
        71 => "D32",
        73 => "D15S1",
        75 => "D24S8",
        77 => "D24X8",
        79 => "D24X4S4",
        80 => "D16",
        100 => "VERTEXDATA",
        101 => "INDEX16",
        102 => "INDEX32",
        _ => {
            let bytes = format.to_le_bytes();
            if bytes.iter().all(|b| b.is_ascii_alphanumeric()) {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            return format!("FMT({format})");
        }
    };
    name.to_string()
}

