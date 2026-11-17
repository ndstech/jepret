//! After-capture workflow runner. Owner: **T04**. See PRD F7.
//!
//! Rules:
//! * Actions run in order; a failing action is recorded in
//!   [`WorkflowOutcome::errors`] and the rest still run.
//! * If any action [`Action::needs_file`] and no file exists yet, the image
//!   is first written to `paths::temp_dir()`. (If `save` is in the list, it is
//!   run early instead, so the real output file is used.)
//! * Does **not** touch history — the caller (app) inserts into history
//!   using `WorkflowOutcome::saved_path`.
//!
//! ## `notify` decision
//! A WinRT toast needs a registered AUMID (Start-menu shortcut) which an
//! unpackaged portable exe does not have. So `notify` does **not** show
//! anything here; it fills [`WorkflowOutcome::notification`] with a message
//! and the daemon (T12) shows it as a tray balloon (`Shell_NotifyIcon`).

// `as_chunks` needs Rust 1.88; workspace MSRV is 1.80.
#![allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use chrono::{Datelike, Local, NaiveDateTime};
use jepret_core::config::ImageFormat;
use jepret_core::{paths, template, Action, CaptureResult, CommandSpec, Config, Result};

mod registry;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub struct WorkflowContext<'a> {
    pub config: &'a Config,
    /// Path of `jepret.exe`, used to spawn `--editor`.
    pub exe_path: PathBuf,
}

#[derive(Debug, Default)]
pub struct WorkflowOutcome {
    /// Final file on disk (output dir if `save` ran, else temp), if any.
    pub saved_path: Option<PathBuf>,
    /// True if the file is in the real output dir (should go to history).
    pub saved_permanently: bool,
    /// (action label, error message)
    pub errors: Vec<(String, String)>,
    /// Contract addition (T04): text for the `notify` action. The caller
    /// shows it (tray balloon) since the runner cannot show a toast without
    /// an AUMID. `None` if `notify` was not requested.
    pub notification: Option<String>,
}

/// Run `actions` for an image capture.
pub fn run(
    ctx: &WorkflowContext<'_>,
    capture: &CaptureResult,
    actions: &[Action],
) -> Result<WorkflowOutcome> {
    let image = &capture.image;
    let dims = Some((image.width, image.height));
    let mut out = WorkflowOutcome::default();
    let mut save_done = false;

    for action in actions {
        let label = action.label();
        let res: Result<()> = (|| {
            match action {
                Action::Save => {
                    if !save_done {
                        save_done = true;
                        do_save(ctx, capture, &mut out)?;
                    }
                    Ok(())
                }
                Action::Clipboard => copy_image_to_clipboard(image),
                Action::Notify => {
                    out.notification = Some(match &out.saved_path {
                        Some(p) => format!("Saved {}", file_name(p)),
                        None => format!("Captured {}x{}", image.width, image.height),
                    });
                    Ok(())
                }
                _ => {
                    // needs a file: make sure there is one
                    if out.saved_path.is_none() {
                        if actions.contains(&Action::Save) && !save_done {
                            save_done = true;
                            if let Err(e) = do_save(ctx, capture, &mut out) {
                                out.errors.push(("save".into(), format!("{e:#}")));
                            }
                        }
                        if out.saved_path.is_none() {
                            let p = temp_path(ctx.config.general.image_format.extension())?;
                            write_image(image, &p, ctx.config)?;
                            out.saved_path = Some(p);
                            out.saved_permanently = false;
                        }
                    }
                    let path = out.saved_path.clone().context("no file")?;
                    run_file_action(ctx, &path, dims, action, &mut out)
                }
            }
        })();
        if let Err(e) = res {
            tracing::warn!(action = %label, error = %format!("{e:#}"), "workflow action failed");
            out.errors.push((label, format!("{e:#}")));
        }
    }
    Ok(out)
}

/// Run file-based actions for an existing file (e.g. a finished recording).
/// `save` / `clipboard` (image) are skipped for non-images.
pub fn run_for_file(
    ctx: &WorkflowContext<'_>,
    path: &Path,
    actions: &[Action],
) -> Result<WorkflowOutcome> {
    anyhow::ensure!(path.is_file(), "file not found: {}", path.display());
    let mut out = WorkflowOutcome {
        saved_path: Some(path.to_path_buf()),
        saved_permanently: true,
        ..Default::default()
    };
    for action in actions {
        let label = action.label();
        let res = match action {
            Action::Save | Action::Clipboard => Ok(()),
            Action::Notify => {
                out.notification = Some(format!("Saved {}", file_name(path)));
                Ok(())
            }
            _ => run_file_action(ctx, path, None, action, &mut out),
        };
        if let Err(e) = res {
            tracing::warn!(action = %label, error = %format!("{e:#}"), "workflow action failed");
            out.errors.push((label, format!("{e:#}")));
        }
    }
    Ok(out)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn do_save(
    ctx: &WorkflowContext<'_>,
    capture: &CaptureResult,
    out: &mut WorkflowOutcome,
) -> Result<()> {
    let ext = ctx.config.general.image_format.extension();
    let dims = Some((capture.image.width, capture.image.height));
    let path = output_path_at(ctx.config, ext, Local::now().naive_local(), dims)?;
    write_image(&capture.image, &path, ctx.config)?;
    out.saved_path = Some(path);
    out.saved_permanently = true;
    Ok(())
}

fn temp_path(ext: &str) -> Result<PathBuf> {
    let dir = paths::temp_dir();
    paths::ensure_dir(&dir)?;
    let stamp = Local::now().format("%Y-%m-%d_%H-%M-%S");
    unique_path(&dir, &format!("jepret_{stamp}"), ext)
}

fn run_file_action(
    ctx: &WorkflowContext<'_>,
    path: &Path,
    dims: Option<(u32, u32)>,
    action: &Action,
    _out: &mut WorkflowOutcome,
) -> Result<()> {
    match action {
        Action::CopyPath => copy_text(&path.to_string_lossy()),
        Action::OpenEditor => {
            Command::new(&ctx.exe_path)
                .arg("--editor")
                .arg(path)
                .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .spawn()
                .with_context(|| format!("spawn {}", ctx.exe_path.display()))?;
            Ok(())
        }
        Action::OpenFolder => open_folder(path),
        Action::OpenWith(name) => {
            let app = ctx
                .config
                .apps
                .get(name)
                .with_context(|| format!("app `{name}` is not defined under [apps]"))?;
            spawn_program(&app.path, &app.args, path, dims, false)
        }
        Action::RunCommand(CommandSpec {
            path: exe,
            args,
            hidden,
        }) => spawn_program(exe, args, path, dims, *hidden),
        // Save / Clipboard / Notify are handled by the callers.
        _ => Ok(()),
    }
}

/// Placeholder values for command args and filename patterns.
fn vars_for(
    path: &Path,
    dims: Option<(u32, u32)>,
    now: NaiveDateTime,
) -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("file", path.to_string_lossy().into_owned()),
        (
            "folder",
            path.parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        ("filename", file_name(path)),
    ];
    v.extend(time_vars(now));
    if let Some((w, h)) = dims {
        v.push(("width", w.to_string()));
        v.push(("height", h.to_string()));
    }
    v
}

fn time_vars(now: NaiveDateTime) -> Vec<(&'static str, String)> {
    vec![
        ("datetime", now.format("%Y-%m-%d_%H-%M-%S").to_string()),
        ("date", now.format("%Y-%m-%d").to_string()),
        ("time", now.format("%H-%M-%S").to_string()),
    ]
}

/// Expand `args` for `file`. Pure; unit-tested.
pub fn expand_args(
    args: &[String],
    file: &Path,
    dims: Option<(u32, u32)>,
    now: NaiveDateTime,
) -> Vec<String> {
    let vars = vars_for(file, dims, now);
    args.iter().map(|a| template::expand(a, &vars)).collect()
}

fn spawn_program(
    exe: &Path,
    args: &[String],
    file: &Path,
    dims: Option<(u32, u32)>,
    hidden: bool,
) -> Result<()> {
    let args = expand_args(args, file, dims, Local::now().naive_local());
    let mut cmd = Command::new(exe);
    cmd.args(&args);
    let mut flags = 0;
    if hidden {
        flags |= CREATE_NO_WINDOW;
    }
    cmd.creation_flags(flags);
    // Not waited on: the child outlives the workflow (and we must not block the daemon).
    cmd.spawn()
        .with_context(|| format!("cannot start {}", exe.display()))?;
    Ok(())
}

/// Replace characters that are illegal in Windows file names.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() {
        "jepret".into()
    } else {
        s
    }
}

fn unique_path(dir: &Path, stem: &str, ext: &str) -> Result<PathBuf> {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return Ok(first);
    }
    for n in 1..10_000u32 {
        let p = dir.join(format!("{stem}_{n}.{ext}"));
        if !p.exists() {
            return Ok(p);
        }
    }
    anyhow::bail!("could not find a free file name for {stem}.{ext}")
}

fn output_path_at(
    config: &Config,
    ext: &str,
    now: NaiveDateTime,
    dims: Option<(u32, u32)>,
) -> Result<PathBuf> {
    let dir = config.image_output_dir(now.year(), now.month());
    let mut vars = time_vars(now);
    if let Some((w, h)) = dims {
        vars.push(("width", w.to_string()));
        vars.push(("height", h.to_string()));
    }
    let stem = sanitize(&template::expand(&config.general.filename_pattern, &vars));
    unique_path(&dir, &stem, ext.trim_start_matches('.'))
}

/// Build a unique output path for a new capture from config
/// (`output_dir`, `monthly_subfolders`, `filename_pattern`, extension).
/// Appends `_1`, `_2`… if the file exists. Does not create the file.
pub fn output_path(config: &Config, ext: &str) -> Result<PathBuf> {
    output_path_at(config, ext, Local::now().naive_local(), None)
}

/// Encode and write `image` in the configured format. Creates parent dirs.
pub fn write_image(image: &jepret_core::Image, path: &Path, config: &Config) -> Result<()> {
    if let Some(dir) = path.parent() {
        paths::ensure_dir(dir)?;
    }
    let file = std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    let w = std::io::BufWriter::new(file);
    let res: Result<()> = match config.general.image_format {
        ImageFormat::Png => {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(w)
                .write_image(
                    &image.pixels,
                    image.width,
                    image.height,
                    image::ExtendedColorType::Rgba8,
                )
                .map_err(Into::into)
        }
        ImageFormat::Jpeg => {
            // JPEG has no alpha: drop it.
            let rgb: Vec<u8> = image
                .pixels
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let q = config.general.jpeg_quality.clamp(1, 100);
            image::codecs::jpeg::JpegEncoder::new_with_quality(w, q)
                .encode(
                    &rgb,
                    image.width,
                    image.height,
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(Into::into)
        }
    };
    if res.is_err() {
        let _ = std::fs::remove_file(path);
    }
    res
}

/// Put an image on the clipboard (PNG + CF_DIBV5 so alpha survives where supported).
/// Uses `arboard`, which publishes DIBV5 (and PNG when available).
pub fn copy_image_to_clipboard(image: &jepret_core::Image) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("open clipboard")?;
    cb.set_image(arboard::ImageData {
        width: image.width as usize,
        height: image.height as usize,
        bytes: std::borrow::Cow::Borrowed(&image.pixels),
    })
    .context("set clipboard image")?;
    Ok(())
}

fn copy_text(text: &str) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("open clipboard")?;
    cb.set_text(text.to_string())
        .context("set clipboard text")?;
    Ok(())
}

/// Open Explorer with `path` selected.
fn open_folder(path: &Path) -> Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems};

    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: standard COM usage. CoInitializeEx may report S_FALSE/RPC_E_CHANGED_MODE;
    // only balance with CoUninitialize when we actually initialised. The PIDL is
    // freed after the call and not used afterwards.
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let pidl = ILCreateFromPathW(&wide);
        let res = if pidl.is_null() {
            Err(anyhow::anyhow!("cannot resolve {}", path.display()))
        } else {
            let r = SHOpenFolderAndSelectItems(pidl, None, 0).map_err(anyhow::Error::from);
            ILFree(Some(pidl));
            r
        };
        if init {
            CoUninitialize();
        }
        res
    }
}

/// Apps registered for "Open with" on `.png` (for the settings UI picker).
#[derive(Debug, Clone)]
pub struct OpenWithCandidate {
    pub name: String,
    pub path: PathBuf,
}

/// Extract the executable path from a shell `open\command` string.
/// Handles quoted paths and unquoted paths containing spaces (cut at `.exe`).
pub fn parse_command_exe(cmd: &str) -> Option<PathBuf> {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(PathBuf::from(&rest[..end]));
    }
    let lower = cmd.to_ascii_lowercase();
    let i = lower.find(".exe")?;
    Some(PathBuf::from(&cmd[..i + 4]))
}

fn pretty_name(exe: &Path) -> String {
    let stem = exe
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut c = stem.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => stem,
    }
}

pub fn open_with_candidates() -> Result<Vec<OpenWithCandidate>> {
    use windows::Win32::System::Registry::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER};

    let mut exes: Vec<PathBuf> = Vec::new();
    // Per-user MRU list: values a,b,c… hold exe names -> HKCR\Applications\<exe>.
    let list = registry::enum_values(
        HKEY_CURRENT_USER,
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.png\OpenWithList",
    );
    for (name, exe) in list {
        if name == "MRUList" || exe.is_empty() {
            continue;
        }
        if let Some(cmd) = registry::default_string(
            HKEY_CLASSES_ROOT,
            &format!(r"Applications\{exe}\shell\open\command"),
        ) {
            exes.extend(parse_command_exe(&cmd));
        }
    }
    // ProgIDs registered for .png. AppX/DelegateExecute handlers have no
    // command line and are skipped.
    for (progid, _) in registry::enum_values(HKEY_CLASSES_ROOT, r".png\OpenWithProgids") {
        if let Some(cmd) =
            registry::default_string(HKEY_CLASSES_ROOT, &format!(r"{progid}\shell\open\command"))
        {
            exes.extend(parse_command_exe(&cmd));
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for exe in exes {
        let lower = file_name(&exe).to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "rundll32.exe" | "dllhost.exe" | "explorer.exe"
        ) || !exe.is_file()
        {
            continue;
        }
        if seen.insert(exe.to_string_lossy().to_ascii_lowercase()) {
            out.push(OpenWithCandidate {
                name: pretty_name(&exe),
                path: exe,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 5)
            .unwrap()
            .and_hms_opt(14, 7, 9)
            .unwrap()
    }

    fn cfg(dir: &Path) -> Config {
        let mut c = Config::default();
        c.general.output_dir = dir.to_path_buf();
        c.general.filename_pattern = "jepret_{datetime}".into();
        c.general.monthly_subfolders = false;
        c
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jepret-wf-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn output_path_pattern_and_suffix() {
        let d = tmp("op");
        let c = cfg(&d);
        let p = output_path_at(&c, "png", now(), None).unwrap();
        assert_eq!(p, d.join("jepret_2026-03-05_14-07-09.png"));
        std::fs::write(&p, b"x").unwrap();
        let p2 = output_path_at(&c, "png", now(), None).unwrap();
        assert_eq!(p2, d.join("jepret_2026-03-05_14-07-09_1.png"));
        std::fs::write(&p2, b"x").unwrap();
        let p3 = output_path_at(&c, "png", now(), None).unwrap();
        assert_eq!(p3, d.join("jepret_2026-03-05_14-07-09_2.png"));
    }

    #[test]
    fn output_path_monthly_and_placeholders() {
        let d = tmp("opm");
        let mut c = cfg(&d);
        c.general.monthly_subfolders = true;
        c.general.filename_pattern = "{date}_{time}_{width}x{height}:x".into();
        let p = output_path_at(&c, "jpg", now(), Some((10, 20))).unwrap();
        assert_eq!(p, d.join("2026-03").join("2026-03-05_14-07-09_10x20_x.jpg"));
    }

    #[test]
    fn expand_args_works() {
        let args = vec![
            "{file}".to_string(),
            "--dir={folder}".into(),
            "{filename}".into(),
            "{width}x{height}".into(),
            "{nope}".into(),
        ];
        let out = expand_args(&args, Path::new("C:/a b/x.png"), Some((3, 4)), now());
        assert_eq!(
            out,
            ["C:/a b/x.png", "--dir=C:/a b", "x.png", "3x4", "{nope}"]
        );
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse_command_exe(r#""C:\P F\a.exe" "%1""#),
            Some(PathBuf::from(r"C:\P F\a.exe"))
        );
        assert_eq!(
            parse_command_exe(r"C:\P F\a.EXE %1"),
            Some(PathBuf::from(r"C:\P F\a.EXE"))
        );
        assert_eq!(parse_command_exe("nothing"), None);
    }

    fn capture() -> CaptureResult {
        let mut img = jepret_core::Image::new(4, 3);
        for p in img.pixels.chunks_exact_mut(4) {
            p.copy_from_slice(&[10, 20, 30, 255]);
        }
        CaptureResult {
            image: img,
            kind: jepret_core::CaptureKind::Fullscreen,
            region: jepret_core::Rect::new(0, 0, 4, 3),
            monitor: None,
            captured_at: std::time::SystemTime::now(),
        }
    }

    fn wait_for(p: &Path) -> bool {
        for _ in 0..100 {
            if p.exists() && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn write_image_png_and_jpeg() {
        let d = tmp("wi");
        let mut c = cfg(&d);
        let cap = capture();
        let p = d.join("a.png");
        write_image(&cap.image, &p, &c).unwrap();
        let back = image::open(&p).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (4, 3));
        c.general.image_format = ImageFormat::Jpeg;
        let j = d.join("a.jpg");
        write_image(&cap.image, &j, &c).unwrap();
        assert!(image::open(&j).is_ok());
    }

    /// Integration: save + copy_path + run_command(cmd /c echo) in a temp dir.
    /// Clipboard needs an interactive desktop session, so it is not asserted
    /// strictly (its error is tolerated and reported).
    #[test]
    fn run_save_copy_path_run_command() {
        let d = tmp("run");
        let c = cfg(&d);
        let ctx = WorkflowContext {
            config: &c,
            exe_path: PathBuf::from("jepret.exe"),
        };
        let marker = d.join("marker.txt");
        let actions = vec![
            Action::Save,
            Action::CopyPath,
            Action::RunCommand(CommandSpec {
                path: PathBuf::from("cmd"),
                args: vec![
                    "/c".into(),
                    "echo".into(),
                    "{filename}".into(),
                    ">".into(),
                    marker.to_string_lossy().into_owned(),
                ],
                hidden: true,
            }),
            Action::Notify,
        ];
        let out = run(&ctx, &capture(), &actions).unwrap();
        let saved = out.saved_path.clone().expect("saved");
        assert!(saved.starts_with(&d) && saved.is_file());
        assert!(out.saved_permanently);
        assert!(out.notification.is_some());
        assert!(
            out.errors.iter().all(|(l, _)| l == "copy_path"),
            "unexpected errors: {:?}",
            out.errors
        );
        assert!(wait_for(&marker), "command did not run");
        let text = std::fs::read_to_string(&marker).unwrap();
        assert!(text.contains(&file_name(&saved)));
    }

    #[test]
    fn failure_does_not_stop_next_actions_and_temp_file_used() {
        let d = tmp("fail");
        let c = cfg(&d);
        let ctx = WorkflowContext {
            config: &c,
            exe_path: PathBuf::from("jepret.exe"),
        };
        let actions = vec![
            Action::OpenWith("missing".into()),
            Action::RunCommand(CommandSpec {
                path: PathBuf::from(r"C:\definitely\not\here.exe"),
                args: vec![],
                hidden: true,
            }),
            Action::Notify,
        ];
        let out = run(&ctx, &capture(), &actions).unwrap();
        assert_eq!(out.errors.len(), 2);
        assert!(!out.saved_permanently);
        let p = out.saved_path.unwrap();
        assert!(p.starts_with(paths::temp_dir()));
        let _ = std::fs::remove_file(p);
        assert!(out.notification.is_some());
    }

    #[test]
    fn run_for_file_skips_save() {
        let d = tmp("rff");
        let c = cfg(&d);
        let ctx = WorkflowContext {
            config: &c,
            exe_path: PathBuf::from("jepret.exe"),
        };
        let f = d.join("v.mp4");
        std::fs::write(&f, b"x").unwrap();
        let out =
            run_for_file(&ctx, &f, &[Action::Save, Action::Clipboard, Action::Notify]).unwrap();
        assert!(out.errors.is_empty());
        assert_eq!(out.saved_path.as_deref(), Some(f.as_path()));
        assert!(run_for_file(&ctx, &d.join("nope.mp4"), &[]).is_err());
    }

    #[test]
    #[ignore = "needs interactive desktop clipboard"]
    fn clipboard_roundtrip() {
        copy_image_to_clipboard(&capture().image).unwrap();
        copy_text("hello").unwrap();
    }

    #[test]
    #[ignore = "opens an Explorer window"]
    fn open_folder_selects() {
        let d = tmp("of");
        let f = d.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        open_folder(&f).unwrap();
    }

    #[test]
    fn candidates_do_not_fail() {
        let c = open_with_candidates().unwrap();
        for x in &c {
            assert!(x.path.is_file());
        }
    }
}
