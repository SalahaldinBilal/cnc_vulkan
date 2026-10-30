//! Pieces every COM object of the backend shares.

use std::ffi::c_void;

use crate::d3d::HRESULT;

pub const S_FALSE: HRESULT = 1;
pub const D3DERR_INVALIDCALL: HRESULT = 0x8876_086C_u32 as i32;
pub const D3DERR_NOTAVAILABLE: HRESULT = 0x8876_086A_u32 as i32;
pub const D3DERR_NOTFOUND: HRESULT = 0x8876_0866_u32 as i32;
pub const D3DERR_MOREDATA: HRESULT = 0x8876_0867_u32 as i32;

/// A method table laid out the way COM expects.
#[repr(transparent)]
pub struct Vtbl<const N: usize>(pub [*const c_void; N]);

unsafe impl<const N: usize> Sync for Vtbl<N> {}

macro_rules! vtbl {
    ($($f:expr),* $(,)?) => {
        $crate::vk::com::Vtbl([$($f as *const ::std::ffi::c_void),*])
    };
}
pub(crate) use vtbl;

/// Data the game can attach to a resource with SetPrivateData.
#[derive(Default)]
pub struct PrivateData(Vec<([u8; 16], Vec<u8>)>);

impl PrivateData {
    pub unsafe fn set(&mut self, guid: *const c_void, data: *const c_void, size: u32, flags: u32) -> HRESULT {
        if guid.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let key = unsafe { *(guid as *const [u8; 16]) };
        self.0.retain(|(k, _)| *k != key);
        // D3DSPD_IUNKNOWN: the data is an interface pointer the resource keeps a reference to.
        let bytes = if flags & 1 != 0 {
            let unknown = unsafe { *(data as *const *mut c_void) };
            if !unknown.is_null() {
                unsafe { add_ref(unknown) };
            }
            (unknown as usize).to_le_bytes().to_vec()
        } else {
            unsafe { std::slice::from_raw_parts(data as *const u8, size as usize) }.to_vec()
        };
        self.0.push((key, bytes));
        crate::d3d::D3D_OK
    }

    pub unsafe fn get(&self, guid: *const c_void, data: *mut c_void, size: *mut u32) -> HRESULT {
        if guid.is_null() || size.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let key = unsafe { *(guid as *const [u8; 16]) };
        let Some((_, bytes)) = self.0.iter().find(|(k, _)| *k == key) else { return D3DERR_NOTFOUND };
        let available = unsafe { *size } as usize;
        unsafe { *size = bytes.len() as u32 };
        if data.is_null() {
            return crate::d3d::D3D_OK;
        }
        if available < bytes.len() {
            return D3DERR_MOREDATA;
        }
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len()) };
        crate::d3d::D3D_OK
    }

    pub unsafe fn free(&mut self, guid: *const c_void) -> HRESULT {
        if guid.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let key = unsafe { *(guid as *const [u8; 16]) };
        let before = self.0.len();
        self.0.retain(|(k, _)| *k != key);
        if self.0.len() == before { D3DERR_NOTFOUND } else { crate::d3d::D3D_OK }
    }
}

/// Calls AddRef on any COM object.
pub unsafe fn add_ref(object: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(*(*(object as *const *const usize)).add(1)) };
    unsafe { f(object) }
}

/// Calls Release on any COM object.
pub unsafe fn release(object: *mut c_void) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(*(*(object as *const *const usize)).add(2)) };
    unsafe { f(object) }
}
