# Jepret — aturan kerja untuk agent

Aplikasi screen capture Windows yang ringan, Rust, MIT. Spesifikasi: [PRD.md](PRD.md). Pembagian kerja: [TASKS.md](TASKS.md).

## Struktur
- `crates/jepret-core` — **kontrak**: tipe, config, state, template, paths. Jangan ubah item publik tanpa mencatat di TASKS.md › Contract changes.
- `crates/jepret-*` — satu crate per fitur; `crates/jepret-app` — binary `jepret.exe` (daemon + mode `--editor/--gallery/--settings`).

## Aturan
1. **Kerjakan hanya file milik ticket-mu** (lihat tabel di TASKS.md). Butuh sesuatu dari crate lain → pakai API stub-nya; jangan edit crate itu.
2. **Signature publik di stub = kontrak.** Boleh menambah item baru; mengubah/menghapus → catat di TASKS.md.
3. **Dependency:** tambahkan di `Cargo.toml` crate-mu. Pakai `xxx.workspace = true` kalau sudah ada di root `[workspace.dependencies]`; kalau belum, tulis versi di crate-mu (jangan edit root — T13 merapikan). Fitur `windows` didaftarkan di crate-mu: `windows = { workspace = true, features = [...] }`.
4. **Ringan itu requirement:** jangan polling di daemon, jangan muat semua data ke memori, jangan tambah dependency besar tanpa alasan. Kode daemon tidak boleh `panic`/`unwrap` pada input runtime — kembalikan `Result`.
5. **Koordinat** selalu piksel fisik virtual desktop; **gambar** selalu `jepret_core::Image` RGBA8 sRGB straight alpha.
6. Logika murni (stitch, tone-map, render editor, cleanup rules, template) dipisah dari Win32/egui dan **wajib ada unit test**.
7. `unsafe` hanya di sekitar panggilan Win32/COM, dengan komentar singkat kenapa aman.
8. Logging pakai `tracing` (`debug!` untuk timing, `warn!` untuk kegagalan yang ditoleransi).

## Perintah
```powershell
cargo check --workspace --all-targets
cargo test -p <crate>
cargo clippy -p <crate> --all-targets -- -D warnings
cargo run -p <crate> --example <name>
cargo build --release -p jepret-app
```
Kerja paralel di folder yang sama → set `CARGO_TARGET_DIR` beda per agent, atau (lebih baik) pakai git worktree sendiri.

## Selesai berarti
AC ticket terpenuhi · check/test/clippy hijau · tidak ada `TODO(Txx)` milikmu yang tersisa · laporan berisi cara uji manual.
