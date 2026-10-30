//! D3D8 formats: how each is laid out in the game's memory and how it is stored on the GPU.

use std::sync::OnceLock;

use ash::vk;

pub const A8R8G8B8: u32 = 21;
pub const X8R8G8B8: u32 = 22;
pub const R5G6B5: u32 = 23;
pub const X1R5G5B5: u32 = 24;
pub const A1R5G5B5: u32 = 25;
pub const A4R4G4B4: u32 = 26;
pub const R3G3B2: u32 = 27;
pub const A8: u32 = 28;
pub const A8R3G3B2: u32 = 29;
pub const X4R4G4B4: u32 = 30;
pub const L8: u32 = 50;
pub const A8L8: u32 = 51;
pub const A4L4: u32 = 52;
pub const V8U8: u32 = 60;
pub const Q8W8V8U8: u32 = 63;
pub const R8G8B8: u32 = 20;
pub const D16_LOCKABLE: u32 = 70;
pub const D32: u32 = 71;
pub const D15S1: u32 = 73;
pub const D24S8: u32 = 75;
pub const D24X8: u32 = 77;
pub const D24X4S4: u32 = 79;
pub const D16: u32 = 80;
pub const INDEX16: u32 = 101;
pub const INDEX32: u32 = 102;
pub const DXT1: u32 = u32::from_le_bytes(*b"DXT1");
pub const DXT2: u32 = u32::from_le_bytes(*b"DXT2");
pub const DXT3: u32 = u32::from_le_bytes(*b"DXT3");
pub const DXT4: u32 = u32::from_le_bytes(*b"DXT4");
pub const DXT5: u32 = u32::from_le_bytes(*b"DXT5");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Copied to the GPU as is.
    Direct,
    /// Expanded to B8G8R8A8 on upload.
    Expand,
    /// 4x4 blocks of the given byte size, copied as is.
    Block(u32),
    Depth,
}

#[derive(Clone, Copy, Debug)]
pub struct Info {
    pub d3d: u32,
    pub kind: Kind,
    /// Bytes per pixel in the game's memory, 0 for block formats.
    pub bytes: u32,
    pub vk: vk::Format,
    pub has_alpha: bool,
}

pub fn info(format: u32) -> Option<Info> {
    use Kind::*;
    let (kind, bytes, vk, has_alpha) = match format {
        A8R8G8B8 => (Direct, 4, vk::Format::B8G8R8A8_UNORM, true),
        X8R8G8B8 => (Direct, 4, vk::Format::B8G8R8A8_UNORM, false),
        R8G8B8 => (Expand, 3, vk::Format::B8G8R8A8_UNORM, false),
        R5G6B5 | X1R5G5B5 | X4R4G4B4 => (Expand, 2, vk::Format::B8G8R8A8_UNORM, false),
        A1R5G5B5 | A4R4G4B4 | A8R3G3B2 | A8L8 => (Expand, 2, vk::Format::B8G8R8A8_UNORM, true),
        R3G3B2 | L8 => (Expand, 1, vk::Format::B8G8R8A8_UNORM, false),
        A8 | A4L4 => (Expand, 1, vk::Format::B8G8R8A8_UNORM, true),
        V8U8 => (Direct, 2, vk::Format::R8G8_SNORM, false),
        Q8W8V8U8 => (Direct, 4, vk::Format::R8G8B8A8_SNORM, true),
        DXT1 => (Block(8), 0, vk::Format::BC1_RGBA_UNORM_BLOCK, true),
        DXT2 | DXT3 => (Block(16), 0, vk::Format::BC2_UNORM_BLOCK, true),
        DXT4 | DXT5 => (Block(16), 0, vk::Format::BC3_UNORM_BLOCK, true),
        D16_LOCKABLE | D32 | D15S1 | D24S8 | D24X8 | D24X4S4 | D16 => (Depth, 4, vk::Format::UNDEFINED, false),
        _ => return None,
    };
    Some(Info { d3d: format, kind, bytes, vk, has_alpha })
}

/// Formats that can be drawn into. All of them are stored as B8G8R8A8.
pub fn renderable(format: u32) -> bool {
    matches!(format, A8R8G8B8 | X8R8G8B8 | R5G6B5 | X1R5G5B5 | A1R5G5B5 | A4R4G4B4 | X4R4G4B4)
}

impl Info {
    pub fn gpu_bytes(&self) -> u32 {
        match self.vk {
            vk::Format::R8G8_SNORM => 2,
            _ => 4,
        }
    }

    /// Bytes per row of the game's copy of one level.
    pub fn pitch(&self, width: u32) -> u32 {
        match self.kind {
            Kind::Block(b) => width.div_ceil(4).max(1) * b,
            _ => width * self.bytes,
        }
    }

    /// Rows of pixels or blocks in one level.
    pub fn rows(&self, height: u32) -> u32 {
        match self.kind {
            Kind::Block(_) => height.div_ceil(4).max(1),
            _ => height,
        }
    }

    pub fn level_size(&self, width: u32, height: u32) -> usize {
        self.pitch(width) as usize * self.rows(height) as usize
    }

    /// Byte offset of a pixel (or the block that holds it) in a level.
    pub fn offset(&self, width: u32, x: u32, y: u32) -> usize {
        match self.kind {
            Kind::Block(b) => (y / 4) as usize * self.pitch(width) as usize + (x / 4 * b) as usize,
            _ => y as usize * self.pitch(width) as usize + (x * self.bytes) as usize,
        }
    }

    /// Size in bytes of a level once on the GPU.
    pub fn gpu_size(&self, width: u32, height: u32) -> usize {
        match self.kind {
            Kind::Block(_) => self.level_size(width, height),
            _ => (width * height * self.gpu_bytes()) as usize,
        }
    }

    /// Writes the GPU form of `rows` rows of `width` pixels from the game's layout.
    pub fn to_gpu(&self, src: &[u8], src_pitch: usize, width: u32, rows: u32, dst: &mut [u8]) {
        match self.kind {
            Kind::Direct | Kind::Block(_) => {
                let row = match self.kind {
                    Kind::Block(b) => (width.div_ceil(4).max(1) * b) as usize,
                    _ => (width * self.bytes) as usize,
                };
                for y in 0..rows as usize {
                    dst[y * row..(y + 1) * row].copy_from_slice(&src[y * src_pitch..y * src_pitch + row]);
                }
            }
            Kind::Expand => {
                let w = width as usize;
                if let Some(table) = self.table16() {
                    for y in 0..rows as usize {
                        let s = &src[y * src_pitch..y * src_pitch + w * 2];
                        let d = &mut dst[y * w * 4..(y + 1) * w * 4];
                        for (p, out) in s.chunks_exact(2).zip(d.chunks_exact_mut(4)) {
                            out.copy_from_slice(&table[u16::from_le_bytes([p[0], p[1]]) as usize]);
                        }
                    }
                    return;
                }
                for y in 0..rows as usize {
                    let s = &src[y * src_pitch..];
                    let d = &mut dst[y * w * 4..(y + 1) * w * 4];
                    for x in 0..w {
                        let bgra = self.expand(s, x);
                        d[x * 4..x * 4 + 4].copy_from_slice(&bgra);
                    }
                }
            }
            Kind::Depth => {}
        }
    }

    /// Every 16-bit pixel value of this format already in B8G8R8A8, for the 16-bit formats the
    /// game uploads most. Built the first time it's needed.
    fn table16(&self) -> Option<&'static [[u8; 4]]> {
        type Table = OnceLock<Box<[[u8; 4]]>>;
        static TABLES: [Table; 5] = [const { OnceLock::new() }; 5];
        let slot = match self.d3d {
            R5G6B5 => 0,
            X1R5G5B5 => 1,
            A1R5G5B5 => 2,
            A4R4G4B4 => 3,
            X4R4G4B4 => 4,
            _ => return None,
        };
        Some(TABLES[slot].get_or_init(|| (0..=u16::MAX).map(|v| self.expand(&v.to_le_bytes(), 0)).collect()))
    }

    fn expand(&self, s: &[u8], x: usize) -> [u8; 4] {
        let p16 = || u16::from_le_bytes([s[x * 2], s[x * 2 + 1]]) as u32;
        let scale = |v: u32, bits: u32| ((v * 255 + ((1 << bits) - 1) / 2) / ((1 << bits) - 1)) as u8;
        match self.d3d {
            R8G8B8 => [s[x * 3], s[x * 3 + 1], s[x * 3 + 2], 255],
            R5G6B5 => {
                let p = p16();
                [scale(p & 31, 5), scale(p >> 5 & 63, 6), scale(p >> 11 & 31, 5), 255]
            }
            X1R5G5B5 | A1R5G5B5 => {
                let p = p16();
                let a = if self.d3d == A1R5G5B5 { if p & 0x8000 != 0 { 255 } else { 0 } } else { 255 };
                [scale(p & 31, 5), scale(p >> 5 & 31, 5), scale(p >> 10 & 31, 5), a]
            }
            A4R4G4B4 | X4R4G4B4 => {
                let p = p16();
                let a = if self.d3d == A4R4G4B4 { scale(p >> 12 & 15, 4) } else { 255 };
                [scale(p & 15, 4), scale(p >> 4 & 15, 4), scale(p >> 8 & 15, 4), a]
            }
            A8R3G3B2 | R3G3B2 => {
                let (p, a) = if self.d3d == A8R3G3B2 { (s[x * 2] as u32, s[x * 2 + 1]) } else { (s[x] as u32, 255) };
                [scale(p & 3, 2), scale(p >> 2 & 7, 3), scale(p >> 5 & 7, 3), a]
            }
            A8 => [0, 0, 0, s[x]],
            L8 => [s[x], s[x], s[x], 255],
            A8L8 => [s[x * 2], s[x * 2], s[x * 2], s[x * 2 + 1]],
            A4L4 => {
                let p = s[x] as u32;
                let l = scale(p & 15, 4);
                [l, l, l, scale(p >> 4, 4)]
            }
            _ => [255, 0, 255, 255],
        }
    }

    /// Writes pixels read back from the GPU (B8G8R8A8) in the game's layout.
    pub fn from_bgra(&self, src: &[u8], width: u32, rows: u32, dst: &mut [u8], dst_pitch: usize) {
        for y in 0..rows as usize {
            let s = &src[y * width as usize * 4..];
            let d = &mut dst[y * dst_pitch..];
            for x in 0..width as usize {
                let (b, g, r, a) = (s[x * 4] as u32, s[x * 4 + 1] as u32, s[x * 4 + 2] as u32, s[x * 4 + 3] as u32);
                match self.d3d {
                    A8R8G8B8 | X8R8G8B8 => d[x * 4..x * 4 + 4].copy_from_slice(&s[x * 4..x * 4 + 4]),
                    R8G8B8 => d[x * 3..x * 3 + 3].copy_from_slice(&s[x * 4..x * 4 + 3]),
                    R5G6B5 => put16(d, x, (r >> 3) << 11 | (g >> 2) << 5 | b >> 3),
                    X1R5G5B5 | A1R5G5B5 => put16(d, x, (a >> 7) << 15 | (r >> 3) << 10 | (g >> 3) << 5 | b >> 3),
                    A4R4G4B4 | X4R4G4B4 => put16(d, x, (a >> 4) << 12 | (r >> 4) << 8 | (g >> 4) << 4 | b >> 4),
                    _ => {}
                }
            }
        }
    }
}

fn put16(d: &mut [u8], x: usize, v: u32) {
    d[x * 2..x * 2 + 2].copy_from_slice(&(v as u16).to_le_bytes());
}

/// Number of mip levels in a full chain down to 1x1.
pub fn full_chain(width: u32, height: u32) -> u32 {
    32 - width.max(height).max(1).leading_zeros()
}

pub fn level_extent(width: u32, height: u32, level: u32) -> (u32, u32) {
    ((width >> level).max(1), (height >> level).max(1))
}
