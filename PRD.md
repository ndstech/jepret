# PRD — Jepret (working name)

> Aplikasi screen capture & recording untuk Windows yang **ringan**, terinspirasi ShareX tapi fokus ke fitur inti.
> Lisensi: MIT · Bahasa: Rust · Platform: Windows 10 (1903+) / 11 · Status: v0 — kerangka
>
> Nama "Jepret" hanya nama kerja; ganti dengan find-replace `jepret` → nama baru.

---

## 1. Latar belakang

ShareX makin sering lag: history dimuat penuh ke memori, overlay region dirender GDI+ di CPU (berat di 4K/multi-monitor), dan ratusan fitur ikut dimuat. Kebutuhan pemakai jauh lebih sempit. Jepret dibangun ulang dari nol dengan prinsip:

1. **Proses resident sekecil mungkin** — hanya tray, hotkey, capture, overlay.
2. **Semua UI berat on-demand di proses terpisah** — editor/galeri/settings dibuka sebagai proses `jepret.exe --editor …`; ditutup = memori kembali ke OS.
3. **Tidak ada polling** — idle CPU 0%.
4. **Data besar tidak pernah dimuat penuh** — history di SQLite, thumbnail lazy.

## 2. Target performa (acceptance global)

| Metrik | Target | Cara ukur |
|---|---|---|
| RAM daemon idle (Private Bytes) | ≤ 20 MB | Task Manager / `Get-Process` setelah 5 menit idle |
| CPU daemon idle | 0% | Process Explorer, 1 menit |
| Hotkey → overlay region tampil | ≤ 100 ms | log `tracing` timestamp |
| Hotkey fullscreen → file tersimpan + clipboard | ≤ 300 ms (2× 4K) | log |
| Buka editor | ≤ 300 ms sampai frame pertama | log |
| Galeri 10.000 entri | buka ≤ 500 ms, scroll 60 fps | manual |
| Ukuran `jepret.exe` (release) | ≤ 15 MB | file size |

## 3. Scope

### In scope (MVP sampai v0.4)
- Capture: fullscreen (semua monitor), monitor aktif, window aktif, region, **last region**, **scroll capture**
- Multi-monitor + **HDR** (tone-map ke SDR)
- Record layar (region / fullscreen) + audio sistem + mikrofon → MP4 (H.264 hardware)
- **Workflow / batch action** per hotkey (save, clipboard, editor, buka di aplikasi lain, run command, dst.)
- Image editor sederhana
- Galeri riwayat + auto cleanup
- Settings UI + config file TOML yang bisa diedit tangan

### Non-goals (sekarang)
- Cross-platform (macOS/Linux)
- Uploader ke ratusan layanan seperti ShareX
- Installer & auto-update (rilis = .exe portable di GitHub Releases)
- Mixed-DPI sempurna (didesain benar sejak awal, dipoles belakangan)

### Lumbung (low priority, setelah v0.4)
- OCR via `Windows.Media.Ocr` (offline, bawaan Windows)
- Share ke Telegram (Bot API `sendPhoto`)
- Share ke WhatsApp / Email via Windows Share sheet (`DataTransferManager`)
- Pin screenshot ke layar (always-on-top)
- Export GIF dari rekaman

Semua item lumbung masuk sebagai **`Action` baru** di workflow — arsitektur tidak berubah.

## 4. Fitur & acceptance criteria

### F1 — Capture fullscreen / monitor / window
- `fullscreen`: seluruh virtual desktop (gabungan semua monitor) jadi satu gambar.
- `active_monitor`: monitor tempat kursor berada.
- `active_window`: window foreground, tanpa bayangan/border DWM bila memungkinkan.
- API: Windows.Graphics.Capture (WGC). Fallback DXGI Desktop Duplication jika WGC tidak tersedia.
- Kursor tidak ikut (configurable `capture.show_cursor`). Border kuning WGC dinonaktifkan (`IsBorderRequired = false`, Win11).
- **AC:** 2 monitor beda resolusi → gambar gabungan posisinya sama dengan tata letak di Display Settings; koordinat negatif (monitor di kiri primary) benar.

### F2 — HDR
- Monitor HDR di-capture sebagai `R16G16B16A16_FLOAT` (scRGB linear), lalu di-tone-map ke sRGB 8-bit.
- SDR white level diambil dari setting Windows (`DISPLAYCONFIG_SDR_WHITE_LEVEL`) kecuali `capture.sdr_white_nits` di-set.
- **AC:** screenshot di monitor HDR tampak sama dengan di layar (tidak pudar / tidak overexposed) saat dibuka di viewer SDR.

### F3 — Capture region
- Hotkey → layar **dibekukan** (capture virtual desktop dulu) → overlay fullscreen di atas semua monitor.
- Overlay digambar Direct2D (GPU). Area luar seleksi digelapkan.
- Fitur overlay: drag untuk pilih, **snap ke window** saat hover (klik tanpa drag = pilih window itu), kaca pembesar + koordinat + ukuran piksel, arrow keys untuk geser 1 px (Shift = resize), Enter konfirmasi, Esc batal, klik kanan batal.
- **AC:** dari hotkey ke overlay tampil ≤ 100 ms; tidak ada geser 1 px di batas monitor.

### F4 — Last region
- Region terakhir disimpan ke `state.toml` (rect virtual desktop + ID monitor).
- Hotkey `last_region` langsung capture tanpa overlay. Kalau belum ada region terakhir → fallback ke mode region.
- Kalau monitor tersebut sudah tidak ada → fallback ke mode region + notifikasi.

### F5 — Scroll capture
- User pilih region (overlay F3) → app kirim scroll (mouse wheel ke window di bawah region) → capture → ulang → **stitch** berdasarkan overlap.
- Stitch: cari offset vertikal dengan mencocokkan hash baris (abaikan N px atas/bawah untuk sticky header/footer).
- Berhenti bila: frame tidak berubah 2× berturut-turut, `max_frames` tercapai, atau user menekan Esc.
- **AC:** halaman web panjang (mis. artikel Wikipedia) ter-stitch tanpa baris dobel/hilang pada ≥ 90% kasus uji.

### F6 — Record layar + audio
- Mode: region (pakai overlay) atau fullscreen monitor aktif. Hotkey sama = toggle start/stop.
- Video: WGC frames → Media Foundation Sink Writer, H.264 hardware (NVENC/QSV/AMF via MFT), fallback software.
- Audio: WASAPI loopback (audio sistem) + WASAPI capture (mic), di-mix ke AAC 48 kHz stereo.
- Indikator rekam: border tipis di sekitar region (tidak ikut terekam) + ikon tray berubah + timer.
- **AC:** rekam 1080p30 selama 10 menit, CPU ≤ 10% di mesin dengan GPU encoder, A/V sync drift ≤ 100 ms.

### F7 — Workflow / batch action
Setiap hotkey punya `capture` + daftar `actions` yang dijalankan berurutan. Kegagalan satu action **tidak** menghentikan action berikutnya (dicatat & dinotifikasi).

| Action | Fungsi |
|---|---|
| `save` | simpan ke `general.output_dir` dengan `general.filename_pattern` |
| `clipboard` | salin gambar ke clipboard |
| `copy_path` | salin path file ke clipboard |
| `open_editor` | buka editor Jepret |
| `open_folder` | buka Explorer dengan file terseleksi |
| `notify` | toast notification dengan thumbnail |
| `{ open_with = "<app>" }` | buka di aplikasi eksternal yang didefinisikan di `[apps.<app>]` |
| `{ run_command = { path, args } }` | jalankan program/script bebas |

- Action yang butuh file (`copy_path`, `open_with`, `run_command`, `open_editor`, `open_folder`) tapi `save` tidak ada di daftar → gambar disimpan ke folder temp dulu.
- Placeholder di `args`: `{file}` `{folder}` `{filename}` `{width}` `{height}` `{datetime}` `{date}` `{time}`.
- Settings UI menawarkan daftar aplikasi dari registry "Open with" untuk `.png` agar tinggal pilih.
- Setiap capture yang tersimpan otomatis masuk history (kalau `history.enabled`).

### F8 — Image editor
- Tools: crop, panah, garis, kotak, elips, teks, highlighter, **blur / pixelate**, **step number** (①②③), freehand pen.
- Objek non-destruktif (bisa dipilih, dipindah, diubah warna/tebal, dihapus) sampai disimpan.
- Undo/redo tak terbatas dalam sesi. Zoom & pan.
- Aksi: Save (timpa), Save As, Copy to clipboard (Ctrl+C), Copy & close (Enter).
- **AC:** buka gambar 4K ≤ 300 ms; anotasi tetap halus (60 fps) saat menggambar.

### F9 — Galeri riwayat
- Grid thumbnail, terbaru di atas, virtual scrolling (hanya yang terlihat yang dirender).
- Filter: gambar/video, favorit, rentang tanggal, cari nama file.
- Aksi per item: buka, buka di editor, copy, copy path, buka folder, favorit, hapus.
- Index di SQLite (`%LOCALAPPDATA%\Jepret\history.db`), thumbnail di `%LOCALAPPDATA%\Jepret\thumbs\`.
- Entri yang filenya sudah hilang ditandai & bisa dibersihkan.

### F10 — Auto cleanup
- Aturan (bisa dikombinasi): `max_age_days`, `max_total_mb` (hapus yang tertua dulu), `keep_favorites` (favorit tidak pernah dihapus), `use_recycle_bin`.
- Jalan saat daemon start (kalau `run_on_startup`) dan tiap 24 jam, di thread latar.
- Laporan: jumlah file & MB yang dihapus, ditulis ke log + notifikasi.

### F11 — Daemon, tray, hotkey
- Single instance (named mutex). Instance kedua dengan argumen CLI meneruskan perintah lalu keluar.
- Tray menu: Capture ▸ (semua mode), Record, Galeri, Settings, Buka folder output, Keluar.
- Hotkey global dari config; konflik hotkey dilaporkan via notifikasi, bukan crash.
- Process DPI awareness: **Per-Monitor V2**.
- CLI: `jepret capture <kind>`, `jepret --editor <file>`, `jepret --gallery`, `jepret --settings`.

### F13 — Shortcut: ramah & bisa diubah
Prinsip: **semua bisa dikerjakan dari keyboard**, default mudah diingat, semua bisa diganti di Settings atau `config.toml`, bentrok dideteksi.

**Hotkey global (default, semua di tombol PrtSc):**

| Tombol | Aksi |
|---|---|
| `PrtSc` | Region → save + clipboard + editor |
| `Ctrl+PrtSc` | Fullscreen (semua monitor) |
| `Alt+PrtSc` | Window aktif |
| `Shift+PrtSc` | Last region |
| `Ctrl+Shift+PrtSc` | Scroll capture |
| `Ctrl+Alt+PrtSc` | Rekam region (tekan lagi = stop) |
| `Alt+Shift+PrtSc` | Rekam monitor aktif (tekan lagi = stop) |

- Format string: `jepret_core::keys::KeyCombo` (lenient: `PrtSc`, `ctrl+=`, `Esc`; disimpan kanonik `ctrl+shift+printscreen`).
- **Windows 11 memakai PrtSc untuk Snipping Tool.** Saat start, daemon membaca `HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled`; kalau aktif, tampilkan notifikasi + tombol yang membuka `ms-settings:easeofaccess-keyboard` agar user mematikannya sendiri (app tidak mengubah setting sistem).
- Hotkey yang gagal didaftarkan (dipakai app lain) → notifikasi menyebut kombinasinya + saran ganti.

**Overlay region (tetap, ditampilkan di hint bar bawah):** `Enter`/`Space`/double-click = konfirmasi · `Esc`/klik kanan = batal · panah = geser 1 px, `Shift`+panah = 10 px · `Ctrl`+panah = resize 1 px, `Ctrl+Shift`+panah = 10 px · `Tab`/`Shift+Tab` = pindah snap window · `F` = seluruh monitor di bawah kursor · `A` = semua monitor · `M` = magnifier on/off · `C` = copy koordinat warna piksel (hex).

**Editor (semua bisa diubah, katalog di `jepret_core::shortcuts::EDITOR_SHORTCUTS`):**
- Tool satu huruf: `V` pilih, `C` crop, `A` panah, `L` garis, `R` kotak, `E` elips, `T` teks, `H` highlighter, `B` blur, `X` pixelate, `N` nomor urut, `P` pen, `I` ambil warna.
- Gaya: `1`–`9` warna palet, `[` / `]` tebal, `F` isi/outline, `S` bayangan.
- Edit: `Ctrl+Z`, `Ctrl+Y`/`Ctrl+Shift+Z`, `Del`, `Ctrl+D` duplikat, `Ctrl+A`, `Ctrl+[` / `Ctrl+]` urutan layer, panah geser (Shift = 10 px), `Ctrl+Shift+N` reset nomor.
- Tampilan: `Ctrl+=`/`Ctrl+-`, `Ctrl+0` pas layar, `Ctrl+1` 100%, `F1` atau `?` cheat-sheet.
- File: `Ctrl+C` copy, `Enter` copy & tutup, `Ctrl+S`, `Ctrl+Shift+S`, `Ctrl+Shift+Enter` simpan & tutup, `Ctrl+Shift+C` copy path, `Ctrl+O` buka di app lain, `Ctrl+Shift+O` buka folder, `Esc` tutup.
- Gestur tetap: `Shift` saat menggambar = lurus 45° / kotak / lingkaran, `Alt` = dari tengah, `Space`+drag = pan, `Ctrl`+scroll = zoom, `Esc` membatalkan seleksi dulu sebelum menutup. Saat mengetik teks, hanya `Esc` dan `Ctrl+Enter` yang aktif.
- Tooltip & menu menampilkan shortcut-nya (`editor.show_shortcut_hints`).

**Settings:** tab "Shortcut" berisi tabel hotkey global + semua shortcut editor, kolom pencarian, tombol rekam kombinasi (tekan tombolnya langsung), reset per baris / reset semua, dan peringatan bentrok (`shortcuts::conflicts`).

### F12 — Settings
- UI egui untuk semua field di `config.toml`: umum, hotkey + workflow, aplikasi eksternal, history/cleanup, recording, capture.
- Simpan ke `config.toml`; daemon reload otomatis (file watcher atau perintah reload via single-instance channel).

## 5. Arsitektur

```
jepret.exe (satu binary, beberapa mode)
│
├─ mode daemon (default, resident)          ← target ≤ 20 MB
│   ├─ tray + hotkey + message loop         [jepret-app]
│   ├─ capture engine (WGC/DXGI, HDR)       [jepret-capture]
│   ├─ overlay region (Direct2D)            [jepret-overlay]
│   ├─ workflow runner                      [jepret-workflow]
│   ├─ history write + cleanup              [jepret-history]
│   ├─ recorder (MF + WASAPI)               [jepret-recorder]
│   └─ scroll capture                       [jepret-scroll]
│
├─ mode --editor <file>   (proses terpisah) [jepret-editor]   egui/eframe
├─ mode --gallery         (proses terpisah) [jepret-gallery]  egui/eframe
└─ mode --settings        (proses terpisah) [jepret-settings] egui/eframe

semua crate bergantung ke [jepret-core]: tipe data, config, state, template
```

**Kenapa UI di proses terpisah:** egui + wgpu memakan puluhan MB. Dengan proses terpisah daemon tetap kecil dan crash di editor tidak mematikan hotkey.

### Konvensi koordinat
- Semua `Rect` dalam **piksel fisik di virtual desktop** (origin = kiri-atas monitor primary; bisa negatif).
- Proses Per-Monitor-DPI-V2 sehingga API Win32 mengembalikan piksel fisik.
- `Image` selalu **RGBA8, sRGB, straight alpha, row-major, stride = width × 4**.

### Lokasi data
| Data | Lokasi |
|---|---|
| Config | `%APPDATA%\Jepret\config.toml` |
| State (last region, dll.) | `%APPDATA%\Jepret\state.toml` |
| History DB | `%LOCALAPPDATA%\Jepret\history.db` |
| Thumbnail | `%LOCALAPPDATA%\Jepret\thumbs\` |
| Log | `%LOCALAPPDATA%\Jepret\logs\` |
| Output default | `%USERPROFILE%\Pictures\Jepret\` |

### Contoh config
```toml
[general]
output_dir = "C:/Users/me/Pictures/Jepret"
filename_pattern = "jepret_{datetime}"
image_format = "png"

[[hotkeys]]
keys = "ctrl+shift+4"
capture = "region"
actions = ["save", "clipboard", "open_editor"]

[[hotkeys]]
keys = "ctrl+shift+5"
capture = "region"
actions = ["save", "clipboard", { open_with = "photocraft" }]

[apps.photocraft]
path = "C:/Program Files/Photocraft/photocraft.exe"
args = ["{file}"]

[history.cleanup]
max_age_days = 30
max_total_mb = 5000
keep_favorites = true
use_recycle_bin = true
```

## 6. Stack & dependency

| Kebutuhan | Crate |
|---|---|
| Win32 / WinRT (WGC, D2D, D3D11, MF, WASAPI) | `windows` |
| Hotkey global | `global-hotkey` |
| Tray | `tray-icon` |
| UI editor/galeri/settings | `eframe` / `egui` |
| PNG/JPEG encode | `image` (fitur `png`, `jpeg` saja) |
| Clipboard | `arboard` (atau Win32 langsung untuk CF_DIB + PNG) |
| History | `rusqlite` (`bundled`) |
| Recycle bin | `trash` |
| Config | `serde` + `toml` |
| Log | `tracing` + `tracing-subscriber` + `tracing-appender` |
| f16 (HDR) | `half` |

Aturan: tambah dependency hanya kalau jelas menghemat waktu; cek dampak ke ukuran binary & RAM daemon.

## 7. Milestone

| Versi | Isi | Catatan |
|---|---|---|
| v0.1 | F1, F2, F3, F4, F7, F11, F13 (global + overlay) | bisa dipakai harian |
| v0.2 | F8, F9, F10, F12, F13 (editor + settings) | |
| v0.3 | F6 | |
| v0.4 | F5 | |
| lumbung | OCR, Telegram, WA/Email, pin, GIF | |

Pembagian kerja paralel ada di [TASKS.md](TASKS.md) — semua crate bisa dikerjakan bersamaan karena kontrak API sudah dikunci di stub.

## 8. Risiko

| Risiko | Mitigasi |
|---|---|
| WGC lambat untuk frame pertama (~50–100 ms) | Pertimbangkan DXGI Desktop Duplication untuk screenshot; WGC untuk recording/window |
| Tone-map HDR beda dari tampilan Windows | Pakai SDR white level sistem; uji di monitor HDR asli |
| Scroll capture gagal di app dengan smooth scroll/lazy load | Delay configurable, deteksi frame tidak berubah, fallback manual |
| Hotkey bentrok dengan app lain | Laporkan, jangan crash; settings menampilkan konflik |
| Egui di proses terpisah lambat start | Ukur; opsi renderer `glow` vs `wgpu` |
