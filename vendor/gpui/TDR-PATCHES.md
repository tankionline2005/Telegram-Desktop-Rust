# GPUI 0.2.2 with our patches

A copy of `gpui` 0.2.2 from crates.io (Apache-2.0, see `LICENSE-APACHE`), used
through `[patch.crates-io]` in the workspace's `Cargo.toml`. The examples and
`Cargo.lock` are left out (their `[[example]]` tables removed from `Cargo.toml`);
`rustfmt.toml` keeps `cargo fmt --all` away from it. Every change is marked with
a `tdr patch` comment.

## Patches

1. `src/platform/windows/directx_atlas.rs`, `DirectXAtlas::remove`: the tile is
   given back to the texture's allocator. In 0.2.2 tiles were never
   deallocated: a texture (4 MB at 1024 x 1024 BGRA) was freed only when every
   image in it had been removed, so each frame of an animated emoji took a new
   place, textures piled up, and any long-lived image (an icon, an emoji, a
   userpic) among the frames kept its whole texture alive.

2. `resources/windows/gpui.manifest.xml` (GPUI embeds the app's manifest): the
   segment heap (`heapType`). The default NT heap keeps freed blocks committed;
   the segment heap has less overhead per block and decommits what is freed.

3. `src/taffy.rs`: `0.0_f32` / `1.0_f32` literals (a future-incompatibility
   warning of newer compilers).

To update GPUI: copy the new release here, drop its examples the same way and
apply the patches again.
