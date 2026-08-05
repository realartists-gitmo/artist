# Artist GPUI fork

Minimal fork of Zed commit `c545fb67d5343681b3017d5a27f6e58dfd580c30`.
Only the four GPUI platform/renderer crates are copied; their remaining Zed
workspace dependencies stay pinned to the same upstream commit. Artist owns
this fork to import Linux DMA-BUF stage buffers into GPUI's Vulkan renderer.
