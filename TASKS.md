# TASKS — pembagian kerja paralel

Kontrak API sudah dikunci di stub tiap crate (`anyhow::bail!("TODO(Txx)")` / `todo!("Txx")`) dan `jepret-core`.
**Semua ticket Wave 1 bisa jalan bersamaan.** Tiap ticket hanya menyentuh file miliknya sendiri, jadi tidak ada konflik edit.

Baca dulu: [PRD.md](PRD.md) (apa & kenapa), [CLAUDE.md](CLAUDE.md) (aturan kerja agent).

## Peta dependency

```
                jepret-core (DONE, kontrak)
                       │
 ┌──────┬──────┬───────┼───────┬──────┬──────┬──────┬──────┬──────┐
T01    T02    T03     T04     T05    T06    T07    T09    T10    T11      ← Wave 1, paralel penuh
capture hdr  overlay workflow history editor stitch recorder gallery settings
 │                                                         (fake data
 └──► T08 scroll driver (kode paralel, uji setelah T01)      dulu, lalu T05)
                       │
                T12 app/daemon/dispatch (kode paralel, uji end-to-end setelah T01+T03+T04)
                       │
                T13 integrasi + ukur performa                ← Wave 2
```

## Wave 1

| ID | Ticket | File milik ticket | Tingkat | Bisa diuji mandiri? |
|---|---|---|---|---|
| T01 | Capture engine | `crates/jepret-capture/**` kecuali `hdr.rs` | sulit (WGC/D3D11) | ya, `examples/` |
| T02 | HDR tone-map | `crates/jepret-capture/src/hdr.rs` | sedang, pure | ya, unit test |
| T03 | Overlay region | `crates/jepret-overlay/**` | sulit (Direct2D) | ya, gambar sintetis |
| T04 | Workflow runner | `crates/jepret-workflow/**` | sedang | ya |
| T05 | History + cleanup | `crates/jepret-history/**` | sedang | ya, unit test |
| T06 | Editor | `crates/jepret-editor/**` | sedang–sulit (egui) | ya, `examples/` |
| T07 | Stitch algoritma | `crates/jepret-scroll/src/stitch.rs` (+ `tests/`) | sedang, pure | ya, unit test |
| T08 | Scroll driver | `crates/jepret-scroll/src/driver.rs` | sedang | setelah T01 |
| T09 | Recorder | `crates/jepret-recorder/**` | sulit (MF/WASAPI) | ya, `examples/` |
| T10 | Galeri | `crates/jepret-gallery/**` | sedang | ya, data palsu |
| T11 | Settings UI | `crates/jepret-settings/**` | mudah–sedang | ya |
| T12 | Daemon/tray/hotkey/dispatch | `crates/jepret-app/**` | sedang | penuh setelah T01/T03/T04 |

---

### T01 — Capture engine
**Implement:** semua fungsi di `jepret-capture/src/lib.rs` kecuali `hdr`.
- `monitors()`: `EnumDisplayMonitors` + `GetMonitorInfoW` + `GetDpiForMonitor`; HDR & SDR white level via `DisplayConfigGetDeviceInfo` (`DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO`, `..._GET_SDR_WHITE_LEVEL`; nits = value/1000*80).
- Screenshot: utamakan **DXGI Desktop Duplication** (latensi rendah) per output; fallback WGC. Untuk monitor HDR minta format FP16 lalu panggil `hdr::tonemap_scrgb_f16` (stub dari T02 boleh masih `todo!` — tes HDR setelah T02).
- BGRA → RGBA, gabungkan ke virtual desktop.
- `capture_foreground_window`: `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` lalu crop dari capture monitor (tanpa shadow).
- `visible_window_rects`: `EnumWindows`, skip invisible / cloaked (`DWMWA_CLOAKED`) / `WS_EX_TOOLWINDOW` / minimized.
- Tambah `examples/capture_all.rs` yang menyimpan PNG tiap monitor + gabungan ke folder temp dan mencetak waktu tiap capture.

**AC:** `cargo run -p jepret-capture --example capture_all` menghasilkan PNG benar di setup 2 monitor (termasuk koordinat negatif); capture fullscreen ≤ 100 ms untuk 2× 1440p setelah warm-up.

### T02 — HDR tone-map
**Implement:** `hdr.rs` (`tonemap_scrgb_f16`, `srgb_oetf`). Tambah dependency `half`.
- Ikuti langkah di doc comment file. Pakai Reinhard extended pada luminance (BT.709) dengan white point = puncak wajar (mis. 4.0 setelah normalisasi) atau algoritma lain yang lebih baik — dokumentasikan pilihan.
- Hormati `row_pitch`.

**AC:** unit test: (a) SDR white (scRGB = sdr_white/80) → 255; (b) 0 → 0; (c) nilai negatif → 0 tanpa panic; (d) highlight 10× tidak clip keras (monoton naik); (e) `row_pitch > width*4` benar. Benchmark sederhana: 4K ≤ 30 ms release.

### T03 — Overlay region
**Implement:** `select_region`, `RegionBorder`.
- Satu window `WS_POPUP` + `WS_EX_TOPMOST | WS_EX_TOOLWINDOW` seukuran `origin`, Direct2D (`ID2D1HwndRenderTarget` atau DXGI swapchain) menggambar `frozen` + lapisan gelap di luar seleksi.
- Shortcut overlay lengkap di PRD F13 (Enter/Space, Esc, panah, Ctrl+panah resize, Tab snap, F, A, M, C) + hint bar di bawah layar.
- Interaksi PRD F3: drag, hover-snap ke `window_rects` (klik tanpa drag = pilih window), magnifier (zoom 8× sekitar kursor + koordinat + ukuran), panah = geser 1 px, Shift+panah = resize, Enter = konfirmasi, Esc / klik kanan = batal, `initial` = seleksi awal.
- `RegionBorder`: window layered click-through (`WS_EX_TRANSPARENT | WS_EX_LAYERED`), dikecualikan dari capture (`SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`).
- Tambah `examples/demo.rs` dengan gambar gradien sintetis (tanpa jepret-capture).

**AC:** window tampil ≤ 50 ms; seleksi akurat ±0 px dibanding koordinat virtual desktop; tidak ada flicker saat drag di 4K.

### T04 — Workflow runner
**Implement:** semua fungsi di `jepret-workflow`. Tambah `image`, `arboard` (atau Win32 clipboard langsung), `chrono`, `windows`.
- Aturan di doc comment crate (urutan, error tidak menghentikan, temp file).
- `output_path`: pattern + placeholder `datetime`=`%Y-%m-%d_%H-%M-%S`, `date`, `time`; `monthly_subfolders`; suffix `_1`… kalau sudah ada. Pakai `jepret_core::template::expand`.
- `open_with` / `run_command`: expand placeholder per arg, `std::process::Command` (tanpa shell), `CREATE_NO_WINDOW` kalau `hidden`. App tidak ditemukan → error di `errors`.
- `open_editor`: spawn `exe_path --editor <file>` detached.
- `open_folder`: `SHOpenFolderAndSelectItems`.
- `notify`: toast WinRT (`Windows.UI.Notifications`) — butuh AUMID; kalau rumit, fallback balloon `Shell_NotifyIcon` diserahkan ke T12 (dokumentasikan).
- `open_with_candidates`: baca `HKCR\.png\OpenWithProgids` + `HKCU\...\FileExts\.png\OpenWithList` → resolve ke exe.

**AC:** unit test `output_path` & expand args; test integrasi `run` dengan `[save, copy_path, run_command(cmd /c echo)]` di temp dir.

### T05 — History + cleanup
**Implement:** `jepret-history`. Tambah `rusqlite` (bundled), `image`, `trash`.
- WAL mode, `busy_timeout`, migrasi via `user_version`. Index di `created_at`, `favorite`.
- `cleanup`: (1) umur > `max_age_days`, (2) lalu kalau total > `max_total_mb` hapus yang tertua, **lewati favorit** bila `keep_favorites`; recycle bin bila `use_recycle_bin`; kumpulkan error, jangan berhenti.
- `thumbnail`: nama file = `<id>_<max_side>.jpg`, regenerasi kalau sumber lebih baru.

**AC:** unit test pakai temp dir: insert/list/paging/search/favorite/delete, cleanup umur, cleanup ukuran, favorit aman, `prune_missing`. `list` 10.000 entri (limit 200) ≤ 5 ms.

### T06 — Editor
**Implement:** `run_editor`. Tambah `eframe`, `egui`, `image`, `arboard`.
- Pisahkan `doc.rs` (model shape + undo/redo, tanpa egui), `render.rs` (flatten shape → `Image`, tanpa egui), `ui.rs`.
- Tools PRD F8. Blur/pixelate diterapkan saat render dari piksel asli.
- **Shortcut (PRD F13):** dispatch berdasarkan id dari `jepret_core::shortcuts::EDITOR_SHORTCUTS`, keymap dari `resolve_editor_keymap(&config.editor.shortcuts)`. Semua id wajib terimplementasi. Gestur tetap (Shift/Alt/Space/Ctrl+wheel, Esc bertingkat, mode ketik teks). Cheat-sheet F1 di-generate dari katalog. Tooltip/menu menampilkan `KeyCombo::pretty()`. Palet dari `config.editor.palette`, tool awal `config.editor.default_tool`. Esc tutup → konfirmasi kalau ada perubahan belum disimpan.
- `examples/open.rs <file>`.

**AC:** unit test `doc` (undo/redo) & `render` (kotak di posisi benar, pixelate mengubah area saja); buka PNG 4K ≤ 300 ms (release).

### T07 — Stitch algoritma
**Implement:** `stitch.rs`. Pure Rust, tanpa dependency baru.
- Hash per baris (mis. FxHash/xxhash sederhana tulis sendiri) → cari `dy` kandidat via pencocokan blok baris → verifikasi piksel dengan toleransi kecil (anti-aliasing) → append.
- Deteksi sticky header/footer otomatis dari 2 frame pertama (baris yang identik di posisi sama).
- Buat generator test: halaman sintetis tinggi (teks/kotak acak + header sticky), potong jadi frame dengan offset scroll acak.

**AC:** ≥ 50 kasus sintetis acak (seed tetap) direkonstruksi persis; kasus `NoChange` & `NoMatch` teruji; 1080p frame ≤ 20 ms per push (release).

### T08 — Scroll driver
**Implement:** `driver.rs` sesuai doc comment. `SendInput` `MOUSEEVENTF_WHEEL`, kembalikan posisi kursor, Esc membatalkan (kembalikan hasil sejauh ini).
- Uji manual setelah T01: halaman browser panjang, Explorer, VS Code.

**AC:** artikel Wikipedia panjang ter-capture utuh; Esc menghentikan ≤ 1 frame.

### T09 — Recorder
**Implement:** `jepret-recorder`.
- Video: WGC `Direct3D11CaptureFramePool` (free-threaded) → crop region di GPU (`CopySubresourceRegion`) → `IMFSinkWriter` H.264 (`MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`), input NV12 via Video Processor MFT atau ARGB32 kalau encoder menerima.
- Audio: WASAPI loopback (render endpoint default) + capture mic; resample ke 48 kHz float stereo; mix; AAC.
- Timestamp dari QPC yang sama untuk audio & video.
- `examples/record10s.rs`.

**AC:** file MP4 bisa diputar di Windows Media Player & VLC, audio sinkron; 1080p30 CPU ≤ 10% dengan GPU encoder; `stop()` selalu menghasilkan file valid.

### T10 — Galeri
**Implement:** `run_gallery`. Tambah `eframe`, `egui`, `image`.
- Grid virtual (egui `ScrollArea::show_rows`), thumbnail di-decode di thread worker + cache LRU tekstur.
- Filter & aksi PRD F9. Tombol "Bersihkan sekarang" memanggil `HistoryDb::cleanup` dan menampilkan `CleanupReport`.
- Mulai dengan trait `Source` + data palsu; ganti ke `HistoryDb` saat T05 merge.

**AC:** 10.000 entri palsu scroll mulus; memori ≤ 150 MB.

### T11 — Settings UI
**Implement:** `run_settings`. Tambah `eframe`, `egui`.
- Tab: Umum · Hotkey & Workflow (list hotkey; tiap hotkey: perekam kombinasi tombol, pilih capture kind, daftar action yang bisa diurutkan) · Aplikasi eksternal (tambah dari file picker atau `open_with_candidates`) · History & Cleanup · Recording · Capture.
- **Tab Shortcut (PRD F13):** tabel hotkey global + semua `EDITOR_SHORTCUTS` (dikelompokkan per `group`), pencarian, tombol "rekam" (tangkap kombinasi yang ditekan), reset per baris & semua, peringatan bentrok (`shortcuts::conflicts`, dan antar hotkey global). Parsing/format pakai `jepret_core::KeyCombo`. Override disimpan ke `config.editor.shortcuts` hanya bila beda dari default.
- Validasi: hotkey duplikat, path app tidak ada. Simpan via `Config::save`.

**AC:** semua field `Config` bisa diubah; file hasil bisa dimuat ulang `Config::load` identik.

### T12 — Daemon / tray / hotkey / dispatch
**Implement:** `jepret-app/src/*`. Tambah `global-hotkey`, `tray-icon`, `tracing-subscriber`, `tracing-appender`, `windows`.
- Lihat doc comment `daemon.rs` & `dispatch.rs`. DPI awareness Per-Monitor V2 di awal `main` (+ app manifest via `build.rs` / `embed-resource`).
- Threading: message loop di main thread (tray + hotkey + overlay). Capture → overlay di main thread, workflow & history di worker thread.
- Last region disimpan ke `AppState`. Recording toggle + `RegionBorder` + ikon tray merah.
- Cleanup harian di thread latar.
- **Shortcut (PRD F13):** parse `HotkeyBinding.keys` dengan `jepret_core::KeyCombo` (jangan parser sendiri). Default baru berbasis PrtSc. Deteksi `PrintScreenKeyForSnippingEnabled` → notifikasi + buka `ms-settings:easeofaccess-keyboard`. Hotkey gagal daftar → notifikasi berisi kombinasinya. Menu tray menampilkan shortcut tiap aksi (`pretty()`).

**AC:** RAM idle ≤ 20 MB (release), CPU idle 0%; semua hotkey default bekerja; instance kedua `jepret capture region` diteruskan ke daemon.

## Wave 2

### T13 — Integrasi & performa
- Rapikan `Cargo.toml` root (pindahkan versi dependency crate ke `[workspace.dependencies]`), selesaikan konflik `Cargo.lock`.
- Ukur semua target di PRD §2, profil yang meleset, perbaiki.
- `cargo clippy --workspace -- -D warnings` bersih. Build release + README cara pakai.

## Wave 3 — Desain visual

### T14 — Desain UI (dikerjakan Claude di Artifact)
**Cara kerja:** desain dibuat oleh Claude lewat **Artifact (tipe Design)**, bukan langsung di kode. Hasilnya direview user, lalu diterjemahkan ke egui/Direct2D oleh ticket T15.

**Layar yang didesain:**
1. Overlay region — gelap luar seleksi, border seleksi, label `W × H`, magnifier + koordinat, hint bar shortcut, outline snap window.
2. Editor — toolbar tool (ikon + tooltip shortcut), panel gaya (palet 9 warna, tebal, isi, bayangan), kanvas + zoom, status bar, cheat-sheet F1, dialog konfirmasi tutup.
3. Galeri — grid thumbnail, filter bar, badge video/favorit/file hilang, menu aksi, panel cleanup + laporan.
4. Settings — 7 tab (Umum, Hotkey & Workflow, Shortcut, Aplikasi eksternal, History & Cleanup, Recording, Capture), perekam kombinasi tombol, penanda bentrok, info box Snipping Tool.
5. Tray menu, balon notifikasi, indikator rekam (border + timer), ikon tray idle/rekam, ikon aplikasi.

**Batasan desain (wajib):**
- Bisa diimplementasi di **egui 0.29** (tanpa efek yang tidak didukung: blur latar, animasi kompleks, font variabel). Overlay & border di Direct2D.
- Nuansa Windows 11 (Fluent): sudut membulat, aksen satu warna, ruang lega; **light & dark**.
- Ringan: tanpa gambar latar besar; ikon sebagai SVG sederhana/icon font yang bisa di-embed.
- Shortcut terlihat di mana-mana (tooltip, menu) — itu identitas aplikasi.
- Bahasa UI: Indonesia.

**Output:** token desain (warna light/dark, radius, spacing, tipografi), mockup tiap layar, set ikon tool (13) + ikon tray/app.

### T15 — Terapkan desain (Wave 3, paralel)
Sumber kebenaran visual: kanvas https://claude.ai/artifact/QJGaMEX5iCXhnkXdVajf1c (7 artboard). Kode kanvas tidak perlu dibuka; semua token sudah ada di crate **`jepret-ui-theme`**:
`tokens::Palette` (LIGHT/DARK), konstanta radius/spacing/ukuran, `install(ctx, config.general.theme)` (font Segoe UI Variable + Cascadia Mono dari sistem, visuals egui), `icons::{Icon, paint, paint_filled}`, `widgets::{kbd, shortcut_hint, primary_button, button, IconButton, toggle, segmented, section_header, banner}`.

Aturan bersama:
- **Jangan hard-code warna** di crate UI — ambil dari `Palette`. Jangan ubah `jepret-ui-theme` kecuali kamu T15a; butuh widget baru → buat lokal di crate-mu dan laporkan.
- Title bar tetap native Windows (tidak membuat title bar custom).
- Ikon masih placeholder (kotak) sampai T15a merge — itu normal.
- Mode tema ikut `config.general.theme` (System/Light/Dark).

| ID | Ticket | File milik ticket |
|---|---|---|
| T15a | Renderer ikon SVG-path + polish widget | `crates/jepret-ui-theme/src/icons.rs`, `widgets.rs` |
| T15b | Editor sesuai artboard "Editor — terang/gelap + cheat-sheet" | `crates/jepret-editor/**` |
| T15c | Galeri sesuai artboard "Galeri riwayat" | `crates/jepret-gallery/**` |
| T15d | Settings sesuai artboard "Settings — tab Shortcut" + pilihan tema | `crates/jepret-settings/**` |
| T15e | Overlay & indikator rekam (Direct2D) sesuai "Overlay region" + "Tray…" | `crates/jepret-overlay/**` |
| T15f | Ikon aplikasi (.ico di exe) + ikon tray idle/rekam + ikon menu tray | `crates/jepret-app/**` |

---

## Cara menjalankan agent paralel

Tiap agent kerja di **git worktree sendiri** (supaya `target/` dan file tidak bentrok), lalu di-merge.

Template prompt untuk tiap agent (ganti `Txx`):

```
Kamu mengerjakan ticket Txx di repo Jepret (Rust, Windows screen capture).
Baca CLAUDE.md, PRD.md, dan bagian Txx di TASKS.md.
Implementasikan HANYA file milik Txx. Jangan ubah signature publik yang
sudah ada di stub; kalau benar-benar perlu, tambahkan item baru dan
catat di TASKS.md › "Contract changes".
Selesai = AC Txx terpenuhi, `cargo check --workspace` hijau,
`cargo test -p <crate>` hijau, `cargo clippy -p <crate>` tanpa warning.
Laporkan: apa yang dikerjakan, apa yang belum, cara menguji manual.
```

## Contract changes
_(catat di sini setiap perubahan/penambahan API publik lintas crate)_

- **core:** `keys::KeyCombo` (parser/format kombinasi tombol), `shortcuts::{EDITOR_SHORTCUTS, resolve_editor_keymap, conflicts}`, `config::EditorConfig` (`Config.editor`: shortcuts override, palette, default_tool, show_shortcut_hints). Default hotkey global diganti ke skema PrtSc (PRD F13).
- **workflow (T04):** `WorkflowOutcome.notification: Option<String>` — `notify` tidak menampilkan toast (exe portable tidak punya AUMID); daemon menampilkannya sebagai balloon tray. Tambahan: `expand_args`.
- **capture (T01):** `ensure_dpi_aware()`, `release_cache()` (daemon memanggilnya setelah 60 s idle), helper murni `bgra_to_rgba`/`blit`/`compose`.
- **history (T05):** `HistoryDb::with_thumbs_dir`, `thumbs_dir`, `refresh_missing`. `HistoryDb` Send, bukan Sync.
- **gallery/settings/overlay/recorder:** hanya modul publik tambahan, signature lama tetap.
- **core (T15):** `config::ThemeMode` + `General.theme` (System/Light/Dark). Crate baru `jepret-ui-theme`.
- **workspace:** `rust-version` 1.88 (dipakai `as_chunks`), release `panic = "unwind"` supaya daemon bisa menangkap panic worker.
- **core/ui-theme/editor:** shortcut baru `tool.textbox` (Shift+T), `tool.callout` (K), `style.toggle_outline` (O); `Icon::{TextBox, Callout}`, `Icon::TOOLS` jadi 15; `fonts::ui_font()` (bytes font UI dipakai bersama egui & renderer editor). Editor: `Tool::{TextBox, Callout}`, `ShapeKind::{TextBox, Callout}`, `Shape.outline`, `Shape::scale`.
