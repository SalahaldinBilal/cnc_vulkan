# Direct3D 8 Usage

Measured with our tracer over 12,100 frames: the main menu with its background battle, then a skirmish. The tracer patched the system D3D8's vtables and was removed with that path once the Vulkan backend replaced it. Its last summary and the game's shader binaries are still in `%LOCALAPPDATA%\cnc-vulkan\zh\trace\` on the development machine, and the 14 shaders are copied into `vk/tests.rs`.

Calls to `SetStreamSource`, `SetIndices` and `SetVertexShader` were not captured because the D3D8 runtime rewrites those vtable slots (see [loading-and-hooking.md](loading-and-hooking.md)).

## Drawing

| Call | Count |
|---|---|
| `DrawIndexedPrimitive` triangle list | 5,911,306 |
| `DrawIndexedPrimitive` triangle strip | 373,350 |
| `DrawPrimitiveUP` triangle strip | 20,592 |

No `DrawPrimitive` and no `DrawIndexedPrimitiveUP`.

Per frame on average: about 750 `SetTransform`, 390 `SetMaterial`, 380 `SetLight`, 610 `SetTextureStageState`, 325 `SetRenderState`, 520 draws.

## Shaders

- One vertex shader: declaration 6 dwords, function 42 dwords. Constants set at registers 4 to 18 and 32 to 33.
- 13 pixel shaders, 11 to 40 dwords each. Constant register 0 only.
- Everything else is fixed function.

## Fixed function state

- Texture stages 0 to 3 carry real work. Stages 4 to 7 only get default settings.
- Color ops: `SELECTARG1`, `SELECTARG2`, `MODULATE`, `MULTIPLYADD`, `DOTPRODUCT3`, `DISABLE`.
- Alpha ops: `SELECTARG1`, `SELECTARG2`, `MODULATE`, `ADD`, `DISABLE`.
- Arguments: `TEXTURE`, `DIFFUSE`, `CURRENT`, `TFACTOR`, `TFACTOR|ALPHAREPLICATE`.
- Texture coordinates: indices 0 to 3, `CAMERASPACEPOSITION` and `CAMERASPACEREFLECTIONVECTOR` generation, `TEXTURETRANSFORMFLAGS = COUNT2`.
- Bump env matrix values are set on stage 0 and 1, but `BUMPENVMAP` is never used as an op.
- Lighting: directional and point lights, up to 4 enabled. Material source from vertex color sometimes.
- Fog: never enabled.
- Alpha test with `GREATEREQUAL` and many reference values. Alpha blending with src blends 1, 2, 5, 7, 9 and dest blends 1, 2, 3, 6, 8.
- Stencil: on for shadows, several ops, compare funcs and masks.
- `COLORWRITEENABLE` 0, 7 (RGB) and 8 (alpha only).
- `ZBIAS` 0 and 8. `CULLMODE` none, CW, CCW.
- Transforms: view, projection, texture 0 to 3, world.
- Clear: always target, depth and stencil together.

## Resources

- 2D textures only. No cube or volume textures.
- Formats: `A8R8G8B8`, `X8R8G8B8`, `R5G6B5`, `A1R5G5B5`, `A4R4G4B4`, `DXT1`, `DXT3`, `DXT5`. Largest: `A8R8G8B8` 2048x1024.
- Render target textures: `A8R8G8B8` and `X8R8G8B8` in the default pool.
- Managed textures are locked whole or by sub-rectangle, flags 0.
- Vertex buffer FVFs: `0x002`, `0x012`, `0x112`, `0x142`, `0x152`, `0x242`, `0x252`, and 0 for raw dynamic buffers.
- Static buffers: managed, write-only. Dynamic buffers: default pool, write-only and dynamic, locked with `DISCARD` or `NOOVERWRITE`, sometimes `NOSYSLOCK`.
- Index buffers: `INDEX16` only.
- About 1.7 `CreateImageSurface` and 2.7 `CopyRects` per frame, with small images (44x45 and 60x66 `R5G6B5`, 64x64 and 128x64 `A8R8G8B8`, `A4R4G4B4`). Probably video frames and cameos. Worth a look for performance.
- Surface locks include `READONLY|NOSYSLOCK`. The game reads back from surfaces.
