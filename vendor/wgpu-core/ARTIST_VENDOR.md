# Artist wgpu-core fork

Fork of `wgpu-core 29.0.4`. Unsafe textures imported through
`Device::create_texture_from_hal` start in `RESOURCE` state because Artist's
Vulkan importer explicitly acquires and transitions DMA-BUF images before
handing them to wgpu.
