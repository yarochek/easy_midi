#![windows_subsystem = "windows"]

use eframe::egui;
use egui::{Color32, FontId, Rounding, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicI32, AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use midir::{MidiInput, MidiInputConnection};
use tray_icon::{TrayIcon, TrayIconBuilder, Icon as TrayIconImg};

use winapi::um::winuser::{
    SendInput, GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    SetWindowsHookExW, UnhookWindowsHookEx, CallNextHookEx, GetMessageW,
    WH_KEYBOARD_LL, WH_MOUSE_LL, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT,
    WM_KEYDOWN, WM_SYSKEYDOWN, WM_LBUTTONDOWN, WM_RBUTTONDOWN,
    LLKHF_INJECTED, LLMHF_INJECTED, MSG,
};
use winapi::shared::minwindef::{LPARAM, LRESULT, WPARAM};
use winapi::shared::windef::HHOOK;

fn build_piano_rgba(size: u32) -> Vec<u8> {
    let s = size as usize;
    let sz = size as f32;
    let mut rgba = vec![0u8; s * s * 4];

    let pad = sz * 0.10;
    let corner = sz * 0.14;
    let kx0 = pad;
    let ky0 = pad;
    let kx1 = sz - pad;
    let ky1 = sz - pad;
    let kw = kx1 - kx0;
    let kh = ky1 - ky0;

    let num_white = 7;
    let wk_w = kw / num_white as f32;
    let bk_w = wk_w * 0.55;
    let bk_h = kh * 0.60;
    let black_after: [i32; 5] = [0, 1, 3, 4, 5];

    let transparent = [0u8, 0, 0, 0];
    let white = [250u8, 250, 252, 255];
    let black = [26u8, 26, 32, 255];
    let border = [50u8, 50, 60, 255];
    let sep = [180u8, 180, 190, 255];
    let bthick = (sz * 0.025).max(1.0);
    let sep_thick = (sz * 0.012).max(0.7);

    for y in 0..s {
        for x in 0..s {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            let dx = if px < kx0 + corner { kx0 + corner - px }
                     else if px > kx1 - corner { px - (kx1 - corner) }
                     else { 0.0 };
            let dy = if py < ky0 + corner { ky0 + corner - py }
                     else if py > ky1 - corner { py - (ky1 - corner) }
                     else { 0.0 };
            let inside = dx * dx + dy * dy <= corner * corner;

            let idx = (y * s + x) * 4;
            if !inside {
                rgba[idx..idx + 4].copy_from_slice(&transparent);
                continue;
            }

            let lx = px - kx0;
            let ly = py - ky0;

            let edge_dist = lx.min(kw - lx).min(ly).min(kh - ly);
            if edge_dist < bthick {
                rgba[idx..idx + 4].copy_from_slice(&border);
                continue;
            }

            let mut is_black = false;
            if ly < bk_h {
                for &bw in &black_after {
                    let bcx = (bw as f32 + 1.0) * wk_w;
                    let bx0 = bcx - bk_w * 0.5;
                    let bx1 = bcx + bk_w * 0.5;
                    if lx >= bx0 && lx < bx1 {
                        is_black = true;
                        break;
                    }
                }
            }

            let color = if is_black {
                black
            } else {
                let wk_idx = (lx / wk_w).floor() as i32;
                if wk_idx > 0 && wk_idx < num_white {
                    let left_edge = wk_idx as f32 * wk_w;
                    if (lx - left_edge).abs() < sep_thick {
                        sep
                    } else {
                        white
                    }
                } else {
                    white
                }
            };

            rgba[idx..idx + 4].copy_from_slice(&color);
        }
    }

    rgba
}

fn build_circle_rgba(size: u32, color: [u8; 3]) -> Vec<u8> {
    let s = size as usize;
    let sz = size as f32;
    let mut rgba = vec![0u8; s * s * 4];
    let cx = sz / 2.0;
    let cy = sz / 2.0;
    let r = sz * 0.40;

    for y in 0..s {
        for x in 0..s {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let a = if d <= r - 0.5 { 1.0 }
                    else if d >= r + 0.5 { 0.0 }
                    else { (r + 0.5 - d).clamp(0.0, 1.0) };
            let idx = (y * s + x) * 4;
            rgba[idx] = color[0];
            rgba[idx + 1] = color[1];
            rgba[idx + 2] = color[2];
            rgba[idx + 3] = (a * 255.0) as u8;
        }
    }
    rgba
}

const TRAY_OFF: [u8; 3] = [220, 60, 60];
const TRAY_ON: [u8; 3] = [60, 200, 80];

fn surface()        -> Color32 { Color32::from_rgb(20, 18, 24) }
fn surface_low()    -> Color32 { Color32::from_rgb(29, 27, 32) }
fn surface_high()   -> Color32 { Color32::from_rgb(43, 41, 48) }
fn surface_highest()-> Color32 { Color32::from_rgb(54, 52, 59) }
fn outline()        -> Color32 { Color32::from_rgb(72, 70, 76) }
fn outline_var()    -> Color32 { Color32::from_rgb(73, 69, 79) }
fn on_surface()     -> Color32 { Color32::from_rgb(230, 224, 233) }
fn on_surface_var() -> Color32 { Color32::from_rgb(202, 196, 208) }

const LYRE_LOW: i32 = 48;
const LYRE_HIGH: i32 = 83;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Instrument {
    #[default]
    Piano,
    GenshinLyre,
}

fn default_true() -> bool { true }
const LYRE_TAP_MS: u64 = 35;

const LYRE_ROWS: [[(u16, &str); 7]; 3] = [
    [(0x5A, "z"), (0x58, "x"), (0x43, "c"), (0x56, "v"), (0x42, "b"), (0x4E, "n"), (0x4D, "m")],
    [(0x41, "a"), (0x53, "s"), (0x44, "d"), (0x46, "f"), (0x47, "g"), (0x48, "h"), (0x4A, "j")],
    [(0x51, "q"), (0x57, "w"), (0x45, "e"), (0x52, "r"), (0x54, "t"), (0x59, "y"), (0x55, "u")],
];

const LYRE_DEGREE: [Option<usize>; 12] = [
    Some(0), None, Some(1), None, Some(2), Some(3), None, Some(4), None, Some(5), None, Some(6),
];

fn is_natural(n: i32) -> bool {
    LYRE_DEGREE[n.rem_euclid(12) as usize].is_some()
}

fn create_lyre_keys() -> Vec<PianoKey> {
    let mut keys = Vec::new();
    for note in LYRE_LOW..=LYRE_HIGH {
        let pc = (note - LYRE_LOW) % 12;
        let oct = ((note - LYRE_LOW) / 12) as usize;
        if let Some(deg) = LYRE_DEGREE[pc as usize] {
            let (vk, ch) = LYRE_ROWS[oct][deg];
            keys.push(PianoKey {
                midi_note: note as u8,
                vk_code: vk,
                display_char: ch.to_string(),
                is_black: false,
            });
        }
    }
    keys
}

#[derive(Clone, Copy)]
struct LyreConfig {
    auto: bool,
    manual_shift: i32,
}

struct LyreReport {
    shift: i32,
    total: usize,
    snapped: usize,
    folded: usize,
}

impl LyreReport {
    fn describe(&self) -> String {
        if self.total == 0 {
            return "No notes".to_string();
        }
        let in_key = (self.total - self.snapped) * 100 / self.total;
        format!(
            "Transpose {:+} st · {}% notes in key · {} octave-folded · {} notes",
            self.shift, in_key, self.folded, self.total
        )
    }
}

fn lyre_best_shift(notes: &[i32]) -> i32 {
    if notes.is_empty() {
        return 0;
    }
    let mut cands: Vec<i32> = vec![0];
    for k in 1..=5 {
        cands.push(k);
        cands.push(-k);
    }
    cands.push(6);

    let mut best_k = 0;
    let mut best_nat = 0usize;
    for &k in &cands {
        let nat = notes.iter().filter(|&&n| is_natural(n + k)).count();
        if nat > best_nat {
            best_nat = nat;
            best_k = k;
        }
    }

    let mut best_o = 0;
    let mut best_out = usize::MAX;
    for o in [0, -1, 1, -2, 2, -3, 3, -4, 4, -5, 5] {
        let sh = best_k + o * 12;
        let out = notes
            .iter()
            .filter(|&&n| {
                let m = n + sh;
                m < LYRE_LOW || m > LYRE_HIGH
            })
            .count();
        if out < best_out {
            best_out = out;
            best_o = o;
        }
    }
    best_k + best_o * 12
}

fn lyre_convert(events: &[NoteEvent], cfg: LyreConfig) -> (Vec<NoteEvent>, LyreReport) {
    let ons: Vec<i32> = events.iter().filter(|e| e.is_on).map(|e| e.note as i32).collect();
    let shift = if cfg.auto { lyre_best_shift(&ons) } else { cfg.manual_shift };

    let mut out: Vec<NoteEvent> = Vec::with_capacity(ons.len());
    let mut snapped = 0usize;
    let mut folded = 0usize;

    for e in events.iter().filter(|e| e.is_on) {
        let mut n = e.note as i32 + shift;
        if n < LYRE_LOW || n > LYRE_HIGH {
            folded += 1;
            while n < LYRE_LOW { n += 12; }
            while n > LYRE_HIGH { n -= 12; }
        }
        if !is_natural(n) {
            snapped += 1;
            n -= 1;
        }
        if out.iter().rev().take(32).any(|o| o.time_us == e.time_us && o.note == n as u8) {
            continue;
        }
        out.push(NoteEvent { time_us: e.time_us, note: n as u8, is_on: true });
    }

    let report = LyreReport { shift, total: ons.len(), snapped, folded };
    (out, report)
}

struct LiveParams {
    lyre: AtomicBool,
    long_notes: AtomicBool,
    shift: AtomicI32,
}

fn bump_atomic(a: &AtomicI32, delta: i32, lo: i32, hi: i32) {
    let v = (a.load(Ordering::Relaxed) + delta).clamp(lo, hi);
    a.store(v, Ordering::Relaxed);
}

fn lyre_map_note(mut n: i32) -> u8 {
    while n < LYRE_LOW { n += 12; }
    while n > LYRE_HIGH { n -= 12; }
    if !is_natural(n) { n -= 1; }
    n as u8
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum BindAction {
    ToggleStartStop,
}

const BIND_COUNT: usize = 1;
const ALL_BINDS: [BindAction; BIND_COUNT] = [
    BindAction::ToggleStartStop,
];

impl BindAction {
    fn label(&self) -> &'static str {
        match self {
            BindAction::ToggleStartStop => "Play / Stop",
        }
    }
}

fn deserialize_keybinds<'de, D>(d: D) -> Result<Vec<Keybind>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Keybind {
    action: BindAction,
    vk: i32,
}

fn vk_name(vk: i32) -> String {
    match vk {
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x30..=0x39 => ((vk as u8) as char).to_string(),
        0x41..=0x5A => ((vk as u8) as char).to_string(),
        0x20 => "Space".into(),
        0x1B => "Esc".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x2E => "Delete".into(),
        0x2D => "Insert".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        _ => format!("VK{:#04X}", vk),
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    accent_color: [u8; 3],
    last_midi_dir: Option<String>,
    synth_long_notes: bool,
    #[serde(deserialize_with = "deserialize_keybinds")]
    keybinds: Vec<Keybind>,
    #[serde(default)]
    instrument: Instrument,
    #[serde(default = "default_true")]
    lyre_auto: bool,
    #[serde(default)]
    lyre_shift: i32,
    #[serde(default)]
    live_instrument: Instrument,
    #[serde(default)]
    live_shift: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            accent_color: [103, 80, 164],
            last_midi_dir: None,
            synth_long_notes: true,
            keybinds: vec![
                Keybind { action: BindAction::ToggleStartStop, vk: 0x71 },
            ],
            instrument: Instrument::Piano,
            lyre_auto: true,
            lyre_shift: 0,
            live_instrument: Instrument::Piano,
            live_shift: 0,
        }
    }
}

impl Settings {
    fn bind_for(&self, action: BindAction) -> Option<i32> {
        self.keybinds.iter().find(|k| k.action == action).map(|k| k.vk)
    }
    fn set_bind(&mut self, action: BindAction, vk: i32) {
        if let Some(k) = self.keybinds.iter_mut().find(|k| k.action == action) {
            k.vk = vk;
        } else {
            self.keybinds.push(Keybind { action, vk });
        }
    }
    fn clear_bind(&mut self, action: BindAction) {
        self.keybinds.retain(|k| k.action != action);
    }
}

fn settings_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("settings.json")
}

fn midi_folder() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).join("midi")
}

fn load_settings() -> Settings {
    if let Ok(s) = std::fs::read_to_string(settings_path()) {
        serde_json::from_str(&s).unwrap_or_default()
    } else {
        Settings::default()
    }
}

fn save_settings(s: &Settings) {
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(settings_path(), json);
    }
}

#[derive(Clone, Debug)]
struct PianoKey {
    midi_note: u8,
    vk_code: u16,
    display_char: String,
    is_black: bool,
}

fn create_virtual_piano_keys() -> Vec<PianoKey> {
    let mut keys = Vec::new();
    let white_chars = vec![
        "1", "2", "3", "4", "5", "6", "7", "8", "9", "0",
        "q", "w", "e", "r", "t", "y", "u", "i", "o", "p",
        "a", "s", "d", "f", "g", "h", "j", "k", "l", "z",
        "x", "c", "v", "b", "n", "m",
    ];

    fn get_vk(c: &str) -> u16 {
        match c.to_uppercase().chars().next().unwrap_or(' ') {
            '1' => 0x31, '2' => 0x32, '3' => 0x33, '4' => 0x34, '5' => 0x35,
            '6' => 0x36, '7' => 0x37, '8' => 0x38, '9' => 0x39, '0' => 0x30,
            'Q' => 0x51, 'W' => 0x57, 'E' => 0x45, 'R' => 0x52, 'T' => 0x54,
            'Y' => 0x59, 'U' => 0x55, 'I' => 0x49, 'O' => 0x4F, 'P' => 0x50,
            'A' => 0x41, 'S' => 0x53, 'D' => 0x44, 'F' => 0x46, 'G' => 0x47,
            'H' => 0x48, 'J' => 0x4A, 'K' => 0x4B, 'L' => 0x4C, 'Z' => 0x5A,
            'X' => 0x58, 'C' => 0x43, 'V' => 0x56, 'B' => 0x42, 'N' => 0x4E, 'M' => 0x4D,
            '!' => 0x31, '@' => 0x32, '$' => 0x34, '%' => 0x35, '^' => 0x36,
            '*' => 0x38, '(' => 0x39,
            _ => 0,
        }
    }

    let pattern = [false, true, false, true, false, false, true, false, true, false, true, false];
    let mut current_midi = 36u8;
    let black_assignments: Vec<(&str, usize)> = vec![
        ("!", 0), ("@", 1), ("$", 3), ("%", 4), ("^", 5),
        ("*", 7), ("(", 8), ("Q", 10), ("W", 11), ("E", 12),
        ("T", 14), ("Y", 15), ("I", 17), ("O", 18), ("P", 19),
        ("S", 21), ("D", 22), ("G", 24), ("H", 25), ("J", 26),
        ("L", 28), ("Z", 29), ("C", 31), ("V", 32), ("B", 33),
    ];

    let mut white_keys_tmp: Vec<(u8, &str, usize)> = Vec::new();
    for i in 0..36usize {
        if i >= white_chars.len() { break; }
        let ch = white_chars[i];
        while pattern[(current_midi % 12) as usize] { current_midi += 1; }
        white_keys_tmp.push((current_midi, ch, i));
        keys.push(PianoKey {
            midi_note: current_midi,
            vk_code: get_vk(ch),
            display_char: ch.to_string(),
            is_black: false,
        });
        current_midi += 1;
    }
    for (ch, w_idx) in black_assignments {
        if let Some(&(w_midi, _, _)) = white_keys_tmp.iter().find(|&&(_, _, idx)| idx == w_idx) {
            keys.push(PianoKey {
                midi_note: w_midi + 1,
                vk_code: get_vk(ch),
                display_char: ch.to_string(),
                is_black: true,
            });
        }
    }
    keys
}

use std::sync::OnceLock;

static USER_INTERRUPT: AtomicBool = AtomicBool::new(false);
static IGNORED_VKS: OnceLock<Mutex<std::collections::HashSet<i32>>> = OnceLock::new();

fn ignored_vks() -> &'static Mutex<std::collections::HashSet<i32>> {
    IGNORED_VKS.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn set_ignored_vks(vks: std::collections::HashSet<i32>) {
    *ignored_vks().lock().unwrap() = vks;
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let data = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let injected = data.flags & LLKHF_INJECTED != 0;
            let vk = data.vkCode as i32;
            let is_bound = ignored_vks().lock().unwrap().contains(&vk);
            if !injected && !is_bound {
                USER_INTERRUPT.store(true, Ordering::Relaxed);
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let msg = wparam as u32;
        if msg == WM_LBUTTONDOWN || msg == WM_RBUTTONDOWN {
            let data = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
            let injected = data.flags & LLMHF_INJECTED != 0;
            if !injected {
                USER_INTERRUPT.store(true, Ordering::Relaxed);
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

fn spawn_input_hook_thread() {
    thread::spawn(|| unsafe {
        let kb_hook: HHOOK = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), std::ptr::null_mut(), 0);
        let mouse_hook: HHOOK = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), std::ptr::null_mut(), 0);

        let mut msg: MSG = mem::zeroed();
        loop {
            let ret = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if ret == 0 || ret == -1 {
                break;
            }
        }

        if !kb_hook.is_null() { UnhookWindowsHookEx(kb_hook); }
        if !mouse_hook.is_null() { UnhookWindowsHookEx(mouse_hook); }
    });
}

const SCAN_LSHIFT: u16 = 0x2A;

unsafe fn press_key(vk_code: u16, is_black: bool, display_char: &str) {
    if vk_code == 0 { return; }
    let need_shift = is_black || ["!", "@", "$", "%", "^", "*", "("].contains(&display_char);
    let scan_code = unsafe { winapi::um::winuser::MapVirtualKeyA(vk_code as u32, 0) as u16 };
    unsafe {
        if need_shift {
            let mut i: INPUT = mem::zeroed();
            i.type_ = INPUT_KEYBOARD;
            i.u.ki_mut().wScan = SCAN_LSHIFT;
            i.u.ki_mut().dwFlags = KEYEVENTF_SCANCODE;
            SendInput(1, &mut i, mem::size_of::<INPUT>() as i32);
        }
        let mut i: INPUT = mem::zeroed();
        i.type_ = INPUT_KEYBOARD;
        i.u.ki_mut().wScan = scan_code;
        i.u.ki_mut().dwFlags = KEYEVENTF_SCANCODE;
        SendInput(1, &mut i, mem::size_of::<INPUT>() as i32);
        if need_shift {
            let mut i: INPUT = mem::zeroed();
            i.type_ = INPUT_KEYBOARD;
            i.u.ki_mut().wScan = SCAN_LSHIFT;
            i.u.ki_mut().dwFlags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
            SendInput(1, &mut i, mem::size_of::<INPUT>() as i32);
        }
    }
}

unsafe fn release_key(vk_code: u16, _is_black: bool, _display_char: &str) {
    if vk_code == 0 { return; }
    let scan_code = unsafe { winapi::um::winuser::MapVirtualKeyA(vk_code as u32, 0) as u16 };
    unsafe {
        let mut i: INPUT = mem::zeroed();
        i.type_ = INPUT_KEYBOARD;
        i.u.ki_mut().wScan = scan_code;
        i.u.ki_mut().dwFlags = KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP;
        SendInput(1, &mut i, mem::size_of::<INPUT>() as i32);
    }
}

unsafe fn click_key(vk_code: u16, is_black: bool, display_char: &str) {
    unsafe {
        press_key(vk_code, is_black, display_char);
        release_key(vk_code, is_black, display_char);
    }
}

unsafe fn release_all_keys(keys: &[PianoKey]) {
    for k in keys {
        unsafe { release_key(k.vk_code, k.is_black, &k.display_char); }
    }
}

#[derive(Clone)]
struct NoteEvent {
    time_us: u64,
    note: u8,
    is_on: bool,
}

fn parse_midi_file(path: &str) -> Result<Vec<NoteEvent>, String> {
    use midly::{Smf, TrackEventKind, MidiMessage, Timing};
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let smf = Smf::parse(&data).map_err(|e| e.to_string())?;
    let ticks_per_beat: u64 = match smf.header.timing {
        Timing::Metrical(t) => t.as_int() as u64,
        _ => 480,
    };
    let mut tempo_map: Vec<(u64, u64)> = vec![(0, 500_000)];
    for track in &smf.tracks {
        let mut tick = 0u64;
        for ev in track {
            tick += ev.delta.as_int() as u64;
            if let TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) = ev.kind {
                tempo_map.push((tick, t.as_int() as u64));
            }
        }
    }
    tempo_map.sort_by_key(|&(t, _)| t);
    tempo_map.dedup_by_key(|e| e.0);
    let ticks_to_us = |target: u64| -> u64 {
        let mut us = 0u64;
        let mut prev_tick = 0u64;
        let mut prev_tempo = 500_000u64;
        for &(t, tempo) in &tempo_map {
            if t >= target { break; }
            us += (t - prev_tick) * prev_tempo / ticks_per_beat;
            prev_tick = t;
            prev_tempo = tempo;
        }
        us += (target - prev_tick) * prev_tempo / ticks_per_beat;
        us
    };
    let mut events: Vec<NoteEvent> = Vec::new();
    for track in &smf.tracks {
        let mut tick = 0u64;
        for ev in track {
            tick += ev.delta.as_int() as u64;
            if let TrackEventKind::Midi { message, .. } = ev.kind {
                match message {
                    MidiMessage::NoteOn { key, vel } => {
                        let v = vel.as_int();
                        events.push(NoteEvent { time_us: ticks_to_us(tick), note: key.as_int(), is_on: v > 0 });
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        events.push(NoteEvent { time_us: ticks_to_us(tick), note: key.as_int(), is_on: false });
                    }
                    _ => {}
                }
            }
        }
    }
    events.sort_by_key(|e| e.time_us);
    Ok(events)
}

fn format_time(us: u64) -> String {
    let total_secs = (us + 1_000) / 1_000_000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{}:{:02}", mins, secs)
}

fn parse_time_input(input: &str) -> Option<u64> {
    let t = input.trim();
    let sep = t.find(|c: char| c == ':' || c == ' ')?;
    let (m_str, rest) = t.split_at(sep);
    let s_str = &rest[1..];
    let all_digits = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit());
    if !all_digits(m_str) || !all_digits(s_str) || m_str.len() > 4 || s_str.len() > 2 {
        return None;
    }
    let m: u64 = m_str.parse().ok()?;
    let sec: u64 = s_str.parse().ok()?;
    if sec >= 60 {
        return None;
    }
    Some((m * 60 + sec) * 1_000_000)
}

#[derive(Clone, PartialEq)]
enum PlayState {
    Idle,
    Playing,
    Paused,
}

struct SharedState {
    play_state: PlayState,
    progress: f32,
    stop_flag: bool,
    pause_flag: bool,
    seek_to: Option<f32>,
    total_duration_us: u64,
    elapsed_us: u64,
    trigger_toggle: bool,
    generation: u64,
}

impl SharedState {
    fn new() -> Self {
        Self {
            play_state: PlayState::Idle,
            progress: 0.0,
            stop_flag: false,
            pause_flag: false,
            seek_to: None,
            total_duration_us: 0,
            elapsed_us: 0,
            trigger_toggle: false,
            generation: 0,
        }
    }
}

#[derive(PartialEq)]
enum Tab {
    AutoPlay,
    Synth,
}

struct BindCapture {
    action: BindAction,
}

struct EasyMidi {
    settings: Settings,
    tab: Tab,
    midi_files: Vec<PathBuf>,
    selected_file: Option<PathBuf>,
    search_query: String,
    shared: Arc<Mutex<SharedState>>,
    speed: Arc<AtomicU32>,
    speed_value: f32,
    speed_input: String,
    color_edit: [f32; 3],
    midi_in_opt: Option<MidiInput>,
    synth_ports: Vec<(String, midir::MidiInputPort)>,
    selected_port_idx: usize,
    synth_conn: Option<MidiInputConnection<()>>,
    settings_popup_open: bool,
    bind_capture: Option<BindCapture>,
    settings_popup_rect: Option<egui::Rect>,
    hotkey_settings: Option<Arc<Mutex<Settings>>>,
    start_progress: f32,
    selected_duration_us: u64,
    list_collapsed: bool,
    tray: Option<TrayIcon>,
    last_tray_state: Option<bool>,
    capturing_flag: Arc<AtomicBool>,
    lyre_info: Option<String>,
    time_input: String,
    live: Arc<LiveParams>,
}

impl EasyMidi {
    fn new(_cc: &eframe::CreationContext) -> Self {
        let mut settings = load_settings();
        settings.lyre_shift = settings.lyre_shift.clamp(-36, 36);
        let live = Arc::new(LiveParams {
            lyre: AtomicBool::new(settings.live_instrument == Instrument::GenshinLyre),
            long_notes: AtomicBool::new(settings.synth_long_notes),
            shift: AtomicI32::new(settings.live_shift.clamp(-36, 36)),
        });
        let color_edit = [
            settings.accent_color[0] as f32 / 255.0,
            settings.accent_color[1] as f32 / 255.0,
            settings.accent_color[2] as f32 / 255.0,
        ];

        let mf = midi_folder();
        if !mf.exists() {
            let _ = std::fs::create_dir_all(&mf);
        }

        let tray = {
            let rgba = build_circle_rgba(32, TRAY_OFF);
            let img = TrayIconImg::from_rgba(rgba, 32, 32).ok();
            let builder = TrayIconBuilder::new().with_tooltip("Easy Midi — Off");
            let builder = if let Some(i) = img { builder.with_icon(i) } else { builder };
            builder.build().ok()
        };

        let speed = Arc::new(AtomicU32::new(1.0f32.to_bits()));

        let mut app = Self {
            settings,
            tab: Tab::AutoPlay,
            midi_files: Vec::new(),
            selected_file: None,
            search_query: String::new(),
            shared: Arc::new(Mutex::new(SharedState::new())),
            speed,
            speed_value: 1.0,
            speed_input: "1.00".to_string(),
            color_edit,
            midi_in_opt: None,
            synth_ports: Vec::new(),
            selected_port_idx: 0,
            synth_conn: None,
            settings_popup_open: false,
            bind_capture: None,
            settings_popup_rect: None,
            hotkey_settings: None,
            start_progress: 0.0,
            selected_duration_us: 0,
            list_collapsed: false,
            tray,
            last_tray_state: None,
            capturing_flag: Arc::new(AtomicBool::new(false)),
            lyre_info: None,
            time_input: String::new(),
            live: Arc::clone(&live),
        };
        app.refresh_midi_list();
        app.refresh_synth_ports();

        let hotkey_shared = Arc::clone(&app.shared);
        let hotkey_settings = Arc::new(Mutex::new(app.settings.clone()));
        let hotkey_capturing = Arc::clone(&app.capturing_flag);
        let hotkey_ctx = _cc.egui_ctx.clone();
        app.hotkey_settings = Some(Arc::clone(&hotkey_settings));
        thread::spawn(move || {
            let mut last_trigger = Instant::now() - Duration::from_secs(10);
            let mut prev_down = [false; BIND_COUNT];
            let mut last_vk: [Option<i32>; BIND_COUNT] = [None; BIND_COUNT];
            loop {
                let binds: Vec<Option<i32>> = {
                    let s = hotkey_settings.lock().unwrap();
                    ALL_BINDS.iter().map(|a| s.bind_for(*a)).collect()
                };
                let capturing = hotkey_capturing.load(Ordering::Relaxed);

                for idx in 0..BIND_COUNT {
                    let vk_opt = binds[idx];
                    let down = match vk_opt {
                        Some(vk) => unsafe { GetAsyncKeyState(vk) as i16 & (1 << 15) != 0 },
                        None => false,
                    };

                    if capturing || vk_opt != last_vk[idx] {
                        last_vk[idx] = vk_opt;
                        prev_down[idx] = down;
                        continue;
                    }

                    let pressed = down && !prev_down[idx];
                    prev_down[idx] = down;
                    if !pressed {
                        continue;
                    }

                    match ALL_BINDS[idx] {
                        BindAction::ToggleStartStop => {
                            if last_trigger.elapsed() > Duration::from_millis(200) {
                                last_trigger = Instant::now();
                                let mut st = hotkey_shared.lock().unwrap();
                                let cur = st.play_state.clone();
                                match cur {
                                    PlayState::Playing => {
                                        st.pause_flag = true;
                                        st.play_state = PlayState::Paused;
                                    }
                                    PlayState::Paused => {
                                        st.pause_flag = false;
                                        st.play_state = PlayState::Playing;
                                    }
                                    PlayState::Idle => {
                                        st.trigger_toggle = true;
                                    }
                                }
                            }
                        }
                    }
                    hotkey_ctx.request_repaint();
                }

                thread::sleep(Duration::from_millis(20));
            }
        });

        set_ignored_vks(app.settings.keybinds.iter().map(|k| k.vk).collect());
        spawn_input_hook_thread();

        app
    }

    fn accent(&self) -> Color32 {
        Color32::from_rgb(
            self.settings.accent_color[0],
            self.settings.accent_color[1],
            self.settings.accent_color[2],
        )
    }

    fn accent_light(&self) -> Color32 {
        let [r, g, b] = self.settings.accent_color;
        Color32::from_rgba_premultiplied(r, g, b, 30)
    }

    fn accent_tonal(&self) -> Color32 {
        let [r, g, b] = self.settings.accent_color;
        Color32::from_rgb(
            ((r as u16 + 60).min(255)) as u8,
            ((g as u16 + 60).min(255)) as u8,
            ((b as u16 + 60).min(255)) as u8,
        )
    }

    fn refresh_midi_list(&mut self) {
        self.midi_files.clear();
        let folder = midi_folder();
        if let Ok(entries) = std::fs::read_dir(&folder) {
            let mut files: Vec<PathBuf> = entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("mid"))
                .collect();
            files.sort();
            self.midi_files = files;
        }
    }

    fn refresh_synth_ports(&mut self) {
        self.synth_ports.clear();
        if let Ok(m) = MidiInput::new("EasyMidi Live") {
            for p in m.ports() {
                if let Ok(name) = m.port_name(&p) {
                    self.synth_ports.push((name, p));
                }
            }
            self.midi_in_opt = Some(m);
            self.selected_port_idx = 0;
        }
    }

    fn sync_hotkey_settings(&self) {
        if let Some(hs) = &self.hotkey_settings {
            *hs.lock().unwrap() = self.settings.clone();
        }
        set_ignored_vks(self.settings.keybinds.iter().map(|k| k.vk).collect());
    }

    fn set_speed(&mut self, s: f32) {
        let clamped = s.clamp(0.1, 5.0);
        self.speed_value = clamped;
        self.speed.store(clamped.to_bits(), Ordering::Relaxed);
    }

    fn lyre_cfg(&self) -> LyreConfig {
        LyreConfig {
            auto: self.settings.lyre_auto,
            manual_shift: self.settings.lyre_shift,
        }
    }

    fn refresh_lyre_report(&mut self) {
        self.lyre_info = None;
        if self.settings.instrument != Instrument::GenshinLyre {
            return;
        }
        let path = match self.selected_file.clone() {
            Some(p) => p,
            None => return,
        };
        if let Ok(ev) = parse_midi_file(path.to_str().unwrap_or("")) {
            let (_, rep) = lyre_convert(&ev, self.lyre_cfg());
            self.lyre_info = Some(rep.describe());
        }
    }

    fn start_playback(&mut self) {
        let path = match &self.selected_file {
            Some(p) => p.clone(),
            None => return,
        };

        let start_progress = if self.start_progress >= 1.0 { 0.0 } else { self.start_progress };
        self.start_progress = 0.0;

        let my_gen = {
            let mut st = self.shared.lock().unwrap();
            st.stop_flag = false;
            st.pause_flag = false;
            st.progress = start_progress;
            st.seek_to = None;
            st.elapsed_us = 0;
            st.generation = st.generation.wrapping_add(1);
            st.generation
        };

        let events = match parse_midi_file(path.to_str().unwrap_or("")) {
            Ok(e) => e,
            Err(_) => {
                let mut st = self.shared.lock().unwrap();
                st.play_state = PlayState::Idle;
                return;
            }
        };

        {
            let mut st = self.shared.lock().unwrap();
            st.play_state = PlayState::Playing;
        }

        let total_us = events.last().map(|e| e.time_us).unwrap_or(1).max(1);

        let tap: Option<Duration> = if self.settings.instrument == Instrument::GenshinLyre {
            Some(Duration::from_millis(LYRE_TAP_MS))
        } else {
            None
        };
        let (events, piano_keys) = if tap.is_some() {
            (lyre_convert(&events, self.lyre_cfg()).0, create_lyre_keys())
        } else {
            (events, create_virtual_piano_keys())
        };

        let shared = Arc::clone(&self.shared);
        let speed = Arc::clone(&self.speed);

        thread::spawn(move || {
            let press = |key: &PianoKey| {
                unsafe { press_key(key.vk_code, key.is_black, &key.display_char); }
            };
            let release = |key: &PianoKey| {
                unsafe { release_key(key.vk_code, key.is_black, &key.display_char); }
            };
            let release_all = |notes_held: &std::collections::HashSet<u8>| {
                for n in notes_held {
                    if let Some(k) = piano_keys.iter().find(|k| k.midi_note == *n) {
                        release(k);
                    }
                }
            };

            let mut cursor_us: u64 = (total_us as f64 * start_progress as f64) as u64;
            let mut i: usize = events.iter().position(|e| e.time_us >= cursor_us).unwrap_or(events.len());

            {
                let mut st = shared.lock().unwrap();
                st.total_duration_us = total_us;
                st.progress = start_progress;
                st.elapsed_us = cursor_us;
            }

            let mut notes_held: std::collections::HashSet<u8> = std::collections::HashSet::new();
            let mut pending: Vec<(Instant, u8)> = Vec::new();
            let mut playback_origin = Instant::now();
            let mut is_paused = false;
            let mut current_speed: f32 = f32::from_bits(speed.load(Ordering::Relaxed)).max(0.05);

            loop {
                if !pending.is_empty() {
                    let now = Instant::now();
                    pending.retain(|&(at, note)| {
                        if at <= now {
                            if notes_held.remove(&note) {
                                if let Some(k) = piano_keys.iter().find(|k| k.midi_note == note) {
                                    release(k);
                                }
                            }
                            false
                        } else {
                            true
                        }
                    });
                }

                let new_speed = f32::from_bits(speed.load(Ordering::Relaxed)).max(0.05);
                if (new_speed - current_speed).abs() > 0.0001 {
                    let now_vt = if is_paused {
                        cursor_us
                    } else {
                        cursor_us + (playback_origin.elapsed().as_micros() as f64 * current_speed as f64) as u64
                    };
                    cursor_us = now_vt;
                    playback_origin = Instant::now();
                    current_speed = new_speed;
                }

                let virtual_time_us = if is_paused {
                    cursor_us
                } else {
                    cursor_us + (playback_origin.elapsed().as_micros() as f64 * current_speed as f64) as u64
                };

                {
                    let mut st = shared.lock().unwrap();
                    if st.generation != my_gen {
                        drop(st);
                        release_all(&notes_held);
                        return;
                    }
                    if st.stop_flag {
                        st.play_state = PlayState::Idle;
                        drop(st);
                        release_all(&notes_held);
                        return;
                    }
                }

                {
                    let mut st = shared.lock().unwrap();
                    if let Some(target_progress) = st.seek_to.take() {
                        drop(st);
                        release_all(&notes_held);
                        notes_held.clear();
                        pending.clear();
                        let new_cursor = (total_us as f64 * target_progress as f64) as u64;
                        cursor_us = new_cursor;
                        playback_origin = Instant::now();
                        i = events.iter().position(|e| e.time_us >= new_cursor).unwrap_or(events.len());
                        let mut st = shared.lock().unwrap();
                        st.progress = target_progress;
                        st.elapsed_us = new_cursor;
                        continue;
                    }
                }

                {
                    let st = shared.lock().unwrap();
                    let pause_requested = st.pause_flag;
                    drop(st);

                    if pause_requested && !is_paused {
                        cursor_us += (playback_origin.elapsed().as_micros() as f64 * current_speed as f64) as u64;
                        is_paused = true;
                        if !notes_held.is_empty() {
                            release_all(&notes_held);
                            notes_held.clear();
                        }
                        pending.clear();
                        continue;
                    } else if !pause_requested && is_paused {
                        playback_origin = Instant::now();
                        is_paused = false;
                    }
                }

                if is_paused {
                    thread::sleep(Duration::from_millis(20));
                    let mut st = shared.lock().unwrap();
                    st.progress = (cursor_us as f32 / total_us as f32).min(1.0);
                    st.elapsed_us = cursor_us;
                    continue;
                }

                if i >= events.len() {
                    if pending.is_empty() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }

                let event = events[i].clone();
                if event.time_us > virtual_time_us {
                    let real_wait_us = ((event.time_us - virtual_time_us) as f64 / current_speed as f64) as u64;
                    let mut wait = Duration::from_micros(real_wait_us.min(50_000));
                    if let Some(next) = pending.iter().map(|p| p.0).min() {
                        wait = wait.min(next.saturating_duration_since(Instant::now()));
                    }
                    thread::sleep(wait);
                    let mut st = shared.lock().unwrap();
                    st.progress = (virtual_time_us as f32 / total_us as f32).min(1.0);
                    st.elapsed_us = virtual_time_us;
                    continue;
                }

                if let Some(key) = piano_keys.iter().find(|k| k.midi_note == event.note) {
                    if event.is_on {
                        if let Some(t) = tap {
                            if notes_held.contains(&event.note) {
                                release(key);
                                pending.retain(|p| p.1 != event.note);
                            }
                            press(key);
                            notes_held.insert(event.note);
                            pending.push((Instant::now() + t, event.note));
                        } else {
                            press(key);
                            notes_held.insert(event.note);
                        }
                    } else if tap.is_none() {
                        release(key);
                        notes_held.remove(&event.note);
                    }
                }

                {
                    let mut st = shared.lock().unwrap();
                    st.progress = (event.time_us as f32 / total_us as f32).min(1.0);
                    st.elapsed_us = event.time_us;
                }
                i += 1;
            }

            release_all(&notes_held);

            let mut st = shared.lock().unwrap();
            if st.generation == my_gen {
                st.play_state = PlayState::Idle;
                st.progress = 1.0;
            }
        });
    }

    fn hard_stop_playback(&mut self) {
        let mut st = self.shared.lock().unwrap();
        st.stop_flag = true;
        st.pause_flag = false;
        st.play_state = PlayState::Idle;
        st.progress = 0.0;
        st.elapsed_us = 0;
        drop(st);
        self.start_progress = 0.0;
    }

    fn stop_keep_position(&mut self) {
        let mut st = self.shared.lock().unwrap();
        if !matches!(st.play_state, PlayState::Playing | PlayState::Paused) {
            return;
        }
        let p = st.progress.clamp(0.0, 1.0);
        st.stop_flag = true;
        st.pause_flag = false;
        st.play_state = PlayState::Idle;
        drop(st);
        self.start_progress = if p >= 1.0 { 0.0 } else { p };
    }

    fn restart_in_place(&mut self) {
        let was_playing = {
            let st = self.shared.lock().unwrap();
            matches!(st.play_state, PlayState::Playing)
        };
        self.stop_keep_position();
        if was_playing {
            self.start_playback();
        }
    }

    fn pause_in_place(&mut self) {
        let mut st = self.shared.lock().unwrap();
        st.pause_flag = true;
        st.play_state = PlayState::Paused;
    }

    fn resume_in_place(&mut self) {
        let mut st = self.shared.lock().unwrap();
        st.pause_flag = false;
        st.play_state = PlayState::Playing;
    }

    fn seek_to(&mut self, progress: f32) {
        let mut st = self.shared.lock().unwrap();
        st.seek_to = Some(progress.clamp(0.0, 1.0));
    }

    fn open_midi_folder(&self) {
        let folder = midi_folder();
        let _ = std::process::Command::new("explorer")
            .arg(folder.to_str().unwrap_or("."))
            .spawn();
    }

    fn update_tray(&mut self) {
        let is_playing = matches!(self.shared.lock().unwrap().play_state, PlayState::Playing);
        if self.last_tray_state == Some(is_playing) {
            return;
        }
        if let Some(tray) = &self.tray {
            let color = if is_playing { TRAY_ON } else { TRAY_OFF };
            let rgba = build_circle_rgba(32, color);
            if let Ok(img) = TrayIconImg::from_rgba(rgba, 32, 32) {
                let _ = tray.set_icon(Some(img));
            }
            let tip = if is_playing { "Easy Midi — On" } else { "Easy Midi — Off" };
            let _ = tray.set_tooltip(Some(tip));
        }
        self.last_tray_state = Some(is_playing);
    }
}

fn md3_card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(surface_low())
        .rounding(Rounding::same(16.0))
        .stroke(Stroke::new(1.0, outline_var()))
        .inner_margin(egui::Margin::same(20.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn md3_chip(ui: &mut egui::Ui, label: &str, active: bool, accent: Color32) -> bool {
    let (bg, fg, stroke) = if active {
        (accent, Color32::WHITE, Stroke::NONE)
    } else {
        (Color32::TRANSPARENT, on_surface(), Stroke::new(1.0, outline()))
    };
    let btn = egui::Button::new(egui::RichText::new(label).color(fg).size(12.0))
        .fill(bg)
        .stroke(stroke)
        .rounding(Rounding::same(8.0))
        .min_size(Vec2::new(0.0, 32.0));
    ui.add(btn).clicked()
}

fn md3_stepper(ui: &mut egui::Ui, id_src: &str, value: &mut i32, min: i32, max: i32, suffix: &str) -> bool {
    let mut changed = false;
    let make_btn = |label: &str| {
        egui::Button::new(egui::RichText::new(label).color(on_surface()).size(15.0).strong())
            .fill(surface_highest())
            .stroke(Stroke::new(1.0, outline()))
            .rounding(Rounding::same(8.0))
            .min_size(Vec2::new(32.0, 32.0))
    };

    if ui.add_enabled(*value > min, make_btn("−")).clicked() {
        *value -= 1;
        changed = true;
    }

    let id = egui::Id::new(id_src);
    let buf_id = id.with("buf");
    let editing = ui.ctx().memory(|m| m.has_focus(id));
    let mut text: String = if editing {
        ui.data_mut(|d| d.get_temp::<String>(buf_id)).unwrap_or_else(|| value.to_string())
    } else {
        format!("{:+}", *value)
    };
    let resp = ui.add_sized(
        [64.0, 32.0],
        egui::TextEdit::singleline(&mut text)
            .id(id)
            .font(FontId::proportional(13.0))
            .horizontal_align(egui::Align::Center)
            .vertical_align(egui::Align::Center)
            .frame(false),
    );
    let border = if resp.has_focus() { ui.visuals().selection.bg_fill } else { outline() };
    ui.painter().rect_stroke(resp.rect, Rounding::same(8.0), Stroke::new(1.0, border));

    if resp.gained_focus() {
        ui.data_mut(|d| d.insert_temp(buf_id, value.to_string()));
    } else if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(buf_id, text.clone()));
    }
    if resp.lost_focus() {
        let parsed = text.trim().trim_start_matches('+').trim().parse::<i32>();
        ui.data_mut(|d| d.remove::<String>(buf_id));
        if let Ok(v) = parsed {
            let v = v.clamp(min, max);
            if v != *value {
                *value = v;
                changed = true;
            }
        }
    }

    if ui.add_enabled(*value < max, make_btn("+")).clicked() {
        *value += 1;
        changed = true;
    }
    ui.label(egui::RichText::new(suffix.trim()).color(on_surface_var()).size(11.0));
    changed
}

fn md3_pill(ui: &mut egui::Ui, label: &str, active: bool, accent: Color32) -> bool {
    let (bg, fg, stroke) = if active {
        (accent, Color32::WHITE, Stroke::NONE)
    } else {
        (Color32::TRANSPARENT, on_surface_var(), Stroke::NONE)
    };
    let btn = egui::Button::new(egui::RichText::new(label).color(fg).size(13.0).strong())
        .fill(bg)
        .stroke(stroke)
        .rounding(Rounding::same(20.0))
        .min_size(Vec2::new(0.0, 36.0));
    ui.add(btn).clicked()
}

impl eframe::App for EasyMidi {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));

        let toggle_pressed = {
            let mut st = self.shared.lock().unwrap();
            let t = st.trigger_toggle;
            st.trigger_toggle = false;
            t
        };

        if toggle_pressed {
            let st_now = self.shared.lock().unwrap().play_state.clone();
            match st_now {
                PlayState::Playing => self.pause_in_place(),
                PlayState::Paused => self.resume_in_place(),
                PlayState::Idle => {
                    if self.selected_file.is_some() {
                        self.start_playback();
                    }
                }
            }
        }

        if USER_INTERRUPT.swap(false, Ordering::Relaxed) {
            let is_playing = {
                let st = self.shared.lock().unwrap();
                matches!(st.play_state, PlayState::Playing)
            };
            if is_playing {
                self.pause_in_place();
            }
        }

        if let Some(capture) = &self.bind_capture {
            self.capturing_flag.store(true, Ordering::Relaxed);
            self.shared.lock().unwrap().trigger_toggle = false;
            let action = capture.action;
            unsafe {
                for vk in 1..255i32 {
                    if vk == 0x01 || vk == 0x02 { continue; }
                    if GetAsyncKeyState(vk) as i16 & (1 << 15) != 0 {
                        self.settings.set_bind(action, vk);
                        self.bind_capture = None;
                        self.sync_hotkey_settings();
                        save_settings(&self.settings);
                        break;
                    }
                }
            }
        } else {
            self.capturing_flag.store(false, Ordering::Relaxed);
        }

        self.update_tray();

        {
            let ls = self.live.shift.load(Ordering::Relaxed);
            if ls != self.settings.live_shift {
                self.settings.live_shift = ls;
                save_settings(&self.settings);
            }
        }

        let accent = self.accent();
        let accent_light = self.accent_light();

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = surface();
        visuals.window_fill = surface();
        visuals.override_text_color = Some(on_surface());
        visuals.widgets.noninteractive.bg_fill = surface_low();
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, outline_var());
        visuals.widgets.noninteractive.rounding = Rounding::same(12.0);
        visuals.widgets.inactive.bg_fill = surface_highest();
        visuals.widgets.inactive.bg_stroke = Stroke::NONE;
        visuals.widgets.inactive.rounding = Rounding::same(12.0);
        visuals.widgets.hovered.bg_fill = surface_highest();
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, outline());
        visuals.widgets.hovered.rounding = Rounding::same(12.0);
        visuals.widgets.active.bg_fill = accent;
        visuals.widgets.active.rounding = Rounding::same(12.0);
        visuals.selection.bg_fill = accent;
        visuals.selection.stroke = Stroke::NONE;
        ctx.set_visuals(visuals);

        egui::TopBottomPanel::top("nav").frame(
            egui::Frame::none()
                .fill(surface_low())
                .inner_margin(egui::Margin::symmetric(20.0, 14.0))
        ).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("🎹 Easy Midi")
                        .size(20.0)
                        .color(accent)
                        .strong()
                );
                ui.add_space(24.0);
                if md3_pill(ui, "Auto Play", self.tab == Tab::AutoPlay, accent) {
                    self.tab = Tab::AutoPlay;
                }
                ui.add_space(6.0);
                if md3_pill(ui, "Live Synth", self.tab == Tab::Synth, accent) {
                    self.tab = Tab::Synth;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("v1.0.0").color(on_surface_var()).size(12.0));
                });
            });
        });

        egui::CentralPanel::default().frame(
            egui::Frame::none()
                .fill(surface())
                .inner_margin(egui::Margin::same(0.0))
        ).show(ctx, |ui| {
            match self.tab {
                Tab::AutoPlay => self.show_auto_play(ui, accent, accent_light),
                Tab::Synth => self.show_synth(ui, accent),
            }
        });

        self.show_settings_popup(ctx, accent);
    }
}

impl EasyMidi {
    fn show_auto_play(&mut self, ui: &mut egui::Ui, accent: Color32, accent_light: Color32) {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(false), |ui| {
            let mut panel = egui::SidePanel::left("file_panel")
                .frame(egui::Frame::none().fill(surface_low()).inner_margin(egui::Margin::same(0.0)));
            if self.list_collapsed {
                panel = panel.resizable(false).exact_width(48.0);
            } else {
                panel = panel
                    .resizable(true)
                    .default_width(280.0)
                    .min_width(170.0)
                    .max_width(560.0);
            }
            panel.show_inside(ui, |ui| {
                if self.list_collapsed {
                    ui.add_space(12.0);
                    ui.vertical_centered(|ui| {
                        if ui.add(
                            egui::Button::new(egui::RichText::new("▶").size(16.0).color(accent))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::splat(32.0))
                        ).on_hover_text("Expand file list").clicked() {
                            self.list_collapsed = false;
                        }
                    });
                } else {
                    self.show_file_panel(ui, accent, accent_light);
                }
            });

            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(surface()).inner_margin(egui::Margin::same(0.0)))
                .show_inside(ui, |ui| {
                    self.show_controls(ui, accent, accent_light);
                });
        });
    }

    fn show_file_panel(&mut self, ui: &mut egui::Ui, accent: Color32, accent_light: Color32) {
        egui::Frame::none()
            .fill(surface_low())
            .inner_margin(egui::Margin::symmetric(12.0, 12.0))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(
                            egui::Button::new(egui::RichText::new("◀").size(12.0).color(accent))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::splat(24.0))
                        ).on_hover_text("Collapse file list").clicked() {
                            self.list_collapsed = true;
                        }
                        if ui.add(
                            egui::Button::new(egui::RichText::new("🔄").size(13.0).color(accent))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::splat(24.0))
                        ).on_hover_text("Refresh").clicked() {
                            self.refresh_midi_list();
                        }
                        if ui.add(
                            egui::Button::new(egui::RichText::new("📂").size(13.0).color(accent))
                                .fill(Color32::TRANSPARENT)
                                .stroke(Stroke::NONE)
                                .min_size(Vec2::splat(24.0))
                        ).on_hover_text("Open midi folder").clicked() {
                            self.open_midi_folder();
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new("MIDI FILES").color(on_surface_var()).size(10.0).strong()
                                ).truncate(true)
                            );
                        });
                    });
                });

                ui.add_space(8.0);
                let avail = ui.available_width();
                let search = egui::TextEdit::singleline(&mut self.search_query)
                    .hint_text("Search...")
                    .desired_width(avail)
                    .font(FontId::proportional(13.0));
                ui.add(search);
            });

        ui.add_space(4.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(0.0);
                let q = self.search_query.to_lowercase();
                let files: Vec<PathBuf> = self.midi_files.iter()
                    .filter(|p| {
                        if q.is_empty() { return true; }
                        p.file_stem()
                            .and_then(|s| s.to_str())
                            .map(|s| s.to_lowercase().contains(&q))
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();

                if files.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("No .mid files found").color(on_surface_var()).size(12.0));
                    });
                }

                for path in files {
                    let name = path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("?")
                        .to_string();
                    let is_selected = self.selected_file.as_ref() == Some(&path);

                    let (fill, text_col) = if is_selected {
                        (accent, Color32::WHITE)
                    } else {
                        (Color32::TRANSPARENT, on_surface())
                    };

                    let resp = egui::Frame::none()
                        .fill(fill)
                        .rounding(Rounding::same(8.0))
                        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                        .show(ui, |ui| {
                            ui.set_min_width(0.0);
                            let avail = ui.available_width();
                            ui.allocate_ui_with_layout(
                                Vec2::new(avail, 0.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    let icon_col = if is_selected { Color32::WHITE } else { accent };
                                    ui.label(egui::RichText::new("🎵").color(icon_col).size(11.0));
                                    ui.add_space(2.0);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&name).color(text_col).size(12.0)
                                        ).truncate(true)
                                    );
                                },
                            );
                        });

                    if resp.response.interact(egui::Sense::click()).clicked() {
                        if self.selected_file.as_ref() != Some(&path) {
                            self.hard_stop_playback();
                        }
                        self.selected_file = Some(path.clone());
                        self.selected_duration_us = parse_midi_file(path.to_str().unwrap_or(""))
                            .ok()
                            .and_then(|ev| ev.last().map(|e| e.time_us))
                            .unwrap_or(0);
                        self.refresh_lyre_report();
                    }
                }
            });
    }

    fn show_controls(&mut self, ui: &mut egui::Ui, accent: Color32, accent_light: Color32) {
        egui::Frame::none()
            .inner_margin(egui::Margin { left: 24.0, right: 24.0, top: 24.0, bottom: 80.0 })
            .show(ui, |ui| {
                self.show_controls_inner(ui, accent, accent_light);
            });
    }

    fn show_controls_inner(&mut self, ui: &mut egui::Ui, accent: Color32, _accent_light: Color32) {
        let (play_state, shared_progress, shared_elapsed_us, shared_total_us) = {
            let st = self.shared.lock().unwrap();
            (
                st.play_state.clone(),
                st.progress,
                st.elapsed_us,
                st.total_duration_us,
            )
        };

        let is_idle = matches!(play_state, PlayState::Idle);
        let has_file = self.selected_file.is_some();

        let total_us = if is_idle || shared_total_us == 0 {
            self.selected_duration_us
        } else {
            shared_total_us
        };
        let elapsed_us = if is_idle {
            (total_us as f64 * self.start_progress as f64) as u64
        } else {
            shared_elapsed_us
        };

        md3_card(ui, |ui| {
            match &self.selected_file {
                Some(p) => {
                    let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                    ui.label(egui::RichText::new("NOW PLAYING").color(on_surface_var()).size(10.0).strong());
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(name).size(20.0).color(accent).strong());

                    ui.add_space(10.0);
                    if is_idle {
                        ui.label(egui::RichText::new(
                            "Ready — press hotkey or drag the timeline below"
                        ).color(on_surface_var()).size(12.0));
                    }
                }
                None => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Select a file to play").color(on_surface_var()).size(15.0));
                        ui.add_space(6.0);
                    });
                }
            }
        });

        ui.add_space(16.0);

        let can_seek = has_file && total_us > 0;
        let mut seek_progress = if is_idle { self.start_progress } else { shared_progress };

        ui.horizontal(|ui| {
            let spacing = 12.0;
            let speed_field_w = 64.0;
            let speed_label_w = 46.0;
            let right_total = speed_label_w + 4.0 + speed_field_w;
            let slider_w = (ui.available_width() - right_total - spacing).max(140.0);

            ui.spacing_mut().slider_width = slider_w;
            let seek_bar = egui::Slider::new(&mut seek_progress, 0.0..=1.0).show_value(false);
            let seek_resp = ui.add_enabled(can_seek, seek_bar);
            if seek_resp.changed() && can_seek {
                if is_idle {
                    self.start_progress = seek_progress;
                } else {
                    self.seek_to(seek_progress);
                }
            }

            ui.add_space(spacing);

            ui.label(egui::RichText::new("SPEED").color(on_surface_var()).size(10.0).strong());
            ui.add_space(4.0);
            let resp = ui.add_sized(
                [speed_field_w, 32.0],
                egui::TextEdit::singleline(&mut self.speed_input)
                    .font(FontId::proportional(13.0))
                    .horizontal_align(egui::Align::Center)
                    .vertical_align(egui::Align::Center)
                    .frame(false),
            );
            let border = if resp.has_focus() { ui.visuals().selection.bg_fill } else { outline() };
            ui.painter().rect_stroke(resp.rect, Rounding::same(8.0), Stroke::new(1.0, border));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if let Ok(v) = self.speed_input.trim().trim_end_matches('x').parse::<f32>() {
                    self.set_speed(v);
                    self.speed_input = format!("{:.2}", self.speed_value);
                }
            } else if resp.lost_focus() {
                if let Ok(v) = self.speed_input.trim().trim_end_matches('x').parse::<f32>() {
                    self.set_speed(v);
                }
                self.speed_input = format!("{:.2}", self.speed_value);
            }
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if can_seek {
                let time_id = egui::Id::new("time_input_field");
                let editing = ui.ctx().memory(|m| m.has_focus(time_id));
                if !editing {
                    self.time_input = format_time(elapsed_us);
                }
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.time_input)
                        .id(time_id)
                        .desired_width(48.0)
                        .font(FontId::proportional(13.0)),
                );
                if resp.lost_focus() {
                    if let Some(target_us) = parse_time_input(&self.time_input) {
                        if target_us <= total_us {
                            let p = (target_us as f64 / total_us as f64) as f32;
                            if is_idle {
                                self.start_progress = p;
                            } else {
                                self.seek_to(p);
                            }
                        }
                    }
                    self.time_input = format_time(elapsed_us);
                }
                ui.label(
                    egui::RichText::new(format!("/ {}", format_time(total_us)))
                        .color(on_surface())
                        .size(13.0)
                        .strong(),
                );
            } else {
                ui.label(egui::RichText::new("--:-- / --:--").color(on_surface()).size(13.0).strong());
            }
        });

        ui.add_space(16.0);

        let is_piano = self.settings.instrument == Instrument::Piano;
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("INSTRUMENT").color(on_surface_var()).size(10.0).strong());
            ui.add_space(6.0);
            if md3_chip(ui, "Piano", is_piano, accent) && !is_piano {
                self.settings.instrument = Instrument::Piano;
                save_settings(&self.settings);
                self.restart_in_place();
                self.refresh_lyre_report();
            }
            if md3_chip(ui, "Genshin Lyre", !is_piano, accent) && is_piano {
                self.settings.instrument = Instrument::GenshinLyre;
                save_settings(&self.settings);
                self.restart_in_place();
                self.refresh_lyre_report();
            }
            if !is_piano {
                ui.add_space(12.0);
                if md3_chip(ui, "Auto transpose", self.settings.lyre_auto, accent) {
                    self.settings.lyre_auto = !self.settings.lyre_auto;
                    save_settings(&self.settings);
                    self.restart_in_place();
                    self.refresh_lyre_report();
                }
                if !self.settings.lyre_auto {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("SHIFT").color(on_surface_var()).size(10.0).strong());
                    ui.add_space(4.0);
                    if md3_stepper(ui, "lyre_shift_input", &mut self.settings.lyre_shift, -36, 36, " st") {
                        save_settings(&self.settings);
                        self.restart_in_place();
                        self.refresh_lyre_report();
                    }
                }
            }
        });

        if !is_piano {
            if let Some(info) = &self.lyre_info {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(info).color(on_surface_var()).size(11.0));
            }
        }

        ui.add_space(16.0);

        md3_card(ui, |ui| {
            ui.horizontal(|ui| {
                let (dot_color, status_text, hint) = match &play_state {
                    PlayState::Playing => (
                        Color32::from_rgb(120, 220, 130),
                        "PLAYING",
                        "Press hotkey to pause",
                    ),
                    PlayState::Paused => (
                        Color32::from_rgb(240, 190, 80),
                        "PAUSED",
                        "Press hotkey to resume",
                    ),
                    PlayState::Idle => (
                        Color32::from_rgb(150, 150, 160),
                        "STOPPED",
                        if has_file { "Press hotkey to start" } else { "Select a file first" },
                    ),
                };

                let (rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 6.0, dot_color);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(status_text).color(dot_color).strong().size(14.0));
                ui.add_space(14.0);
                ui.label(egui::RichText::new(hint).color(on_surface_var()).size(12.0));
            });
        });
    }

    fn show_synth(&mut self, ui: &mut egui::Ui, accent: Color32) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(24.0);
            let w = (ui.available_width() - 48.0).min(600.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.set_max_width(w);

                md3_card(ui, |ui| {
                    ui.label(egui::RichText::new("LIVE SYNTHESIZER").color(on_surface_var()).size(10.0).strong());
                    ui.add_space(12.0);

                    ui.label(egui::RichText::new("1. Select MIDI Port").color(on_surface_var()).size(13.0));
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        let selected_name = if self.synth_ports.is_empty() {
                            "No devices found".to_string()
                        } else {
                            self.synth_ports[self.selected_port_idx].0.clone()
                        };

                        let combo_w = (ui.available_width() - 44.0).max(80.0);
                        egui::ComboBox::from_id_source("midi_port_combo")
                            .selected_text(selected_name)
                            .width(combo_w)
                            .show_ui(ui, |ui| {
                                for (i, (name, _)) in self.synth_ports.iter().enumerate() {
                                    ui.selectable_value(&mut self.selected_port_idx, i, name);
                                }
                            });

                        if ui.add(egui::Button::new("🔄")).on_hover_text("Refresh devices").clicked() {
                            self.refresh_synth_ports();
                        }
                    });

                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("2. Instrument").color(on_surface_var()).size(13.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let is_lyre = self.settings.live_instrument == Instrument::GenshinLyre;
                        if md3_chip(ui, "Piano", !is_lyre, accent) && is_lyre {
                            self.settings.live_instrument = Instrument::Piano;
                            self.live.lyre.store(false, Ordering::Relaxed);
                            save_settings(&self.settings);
                            unsafe {
                                release_all_keys(&create_virtual_piano_keys());
                                release_all_keys(&create_lyre_keys());
                            }
                        }
                        ui.add_space(6.0);
                        if md3_chip(ui, "Genshin Lyre", is_lyre, accent) && !is_lyre {
                            self.settings.live_instrument = Instrument::GenshinLyre;
                            self.live.lyre.store(true, Ordering::Relaxed);
                            save_settings(&self.settings);
                            unsafe {
                                release_all_keys(&create_virtual_piano_keys());
                                release_all_keys(&create_lyre_keys());
                            }
                        }
                    });
                    ui.add_space(10.0);

                    if self.settings.live_instrument == Instrument::GenshinLyre {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(egui::RichText::new("SHIFT").color(on_surface_var()).size(10.0).strong());
                            ui.add_space(4.0);
                            let mut sh = self.live.shift.load(Ordering::Relaxed);
                            if md3_stepper(ui, "live_shift_input", &mut sh, -36, 36, " st") {
                                self.live.shift.store(sh, Ordering::Relaxed);
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "Applies instantly, no reconnect. The lyre has only 21 white keys (C3–B5): \
                                 black keys play the white key below, notes outside the range are folded."
                            ).color(on_surface_var()).size(11.0)
                        );
                    } else {
                        ui.horizontal(|ui| {
                            if md3_chip(ui, "Long notes (Hold)", self.settings.synth_long_notes, accent) {
                                self.settings.synth_long_notes = true;
                                self.live.long_notes.store(true, Ordering::Relaxed);
                                save_settings(&self.settings);
                            }
                            ui.add_space(6.0);
                            if md3_chip(ui, "No Hold (tap)", !self.settings.synth_long_notes, accent) {
                                self.settings.synth_long_notes = false;
                                self.live.long_notes.store(false, Ordering::Relaxed);
                                save_settings(&self.settings);
                            }
                        });
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(if self.settings.synth_long_notes {
                                "The key is held for as long as you hold the note on the MIDI device."
                            } else {
                                "Each note is an instant tap — hold duration is ignored."
                            }).color(on_surface_var()).size(11.0)
                        );
                    }

                    ui.add_space(16.0);
                    ui.label(egui::RichText::new("3. Connect & Play").color(on_surface_var()).size(13.0));
                    ui.add_space(8.0);

                    if self.synth_conn.is_some() {
                        ui.label(egui::RichText::new("🟢 CONNECTED").color(Color32::GREEN).strong());
                        ui.add_space(8.0);
                        if ui.button("Disconnect").clicked() {
                            if let Some(conn) = self.synth_conn.take() {
                                conn.close();
                                unsafe {
                                    release_all_keys(&create_virtual_piano_keys());
                                    release_all_keys(&create_lyre_keys());
                                }
                                self.midi_in_opt = MidiInput::new("EasyMidi Live").ok();
                            }
                        }
                    } else {
                        ui.label(egui::RichText::new("🔴 DISCONNECTED").color(Color32::RED).strong());
                        ui.add_space(8.0);

                        let connect_btn = egui::Button::new(
                            egui::RichText::new("Connect Synthesizer").color(Color32::WHITE).size(14.0).strong()
                        )
                        .fill(accent)
                        .rounding(Rounding::same(20.0))
                        .min_size(Vec2::new(0.0, 40.0));

                        if ui.add(connect_btn).clicked() {
                            if !self.synth_ports.is_empty() {
                                if let Some(midi_in) = self.midi_in_opt.take() {
                                    let port = &self.synth_ports[self.selected_port_idx].1;
                                    let piano_keys = create_virtual_piano_keys();
                                    let lyre_keys = create_lyre_keys();
                                    let live = Arc::clone(&self.live);

                                    match midi_in.connect(port, "easy_midi_live", move |_, message, _| {
                                        if message.len() < 3 { return; }
                                        let status = message[0];
                                        let note = message[1];
                                        let velocity = message[2];
                                        let msg_type = status & 0xF0;
                                        let is_on = msg_type == 0x90 && velocity > 0;
                                        let is_off = msg_type == 0x80 || (msg_type == 0x90 && velocity == 0);

                                        if live.lyre.load(Ordering::Relaxed) {
                                            if !is_on { return; }
                                            let total = note as i32 + live.shift.load(Ordering::Relaxed);
                                            let n = lyre_map_note(total);
                                            if let Some(key) = lyre_keys.iter().find(|k| k.midi_note == n) {
                                                let (vk, bl, ch) = (key.vk_code, key.is_black, key.display_char.clone());
                                                unsafe { press_key(vk, bl, &ch); }
                                                thread::spawn(move || {
                                                    thread::sleep(Duration::from_millis(LYRE_TAP_MS));
                                                    unsafe { release_key(vk, bl, &ch); }
                                                });
                                            }
                                            return;
                                        }

                                        if let Some(key) = piano_keys.iter().find(|k| k.midi_note == note) {
                                            if live.long_notes.load(Ordering::Relaxed) {
                                                if is_on {
                                                    unsafe { press_key(key.vk_code, key.is_black, &key.display_char); }
                                                } else if is_off {
                                                    unsafe { release_key(key.vk_code, key.is_black, &key.display_char); }
                                                }
                                            } else if is_on {
                                                unsafe { click_key(key.vk_code, key.is_black, &key.display_char); }
                                            }
                                        }
                                    }, ()) {
                                        Ok(conn) => self.synth_conn = Some(conn),
                                        Err(_) => {
                                            self.midi_in_opt = MidiInput::new("EasyMidi Live").ok();
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            });
        });
    }

    fn show_settings_popup(&mut self, ctx: &egui::Context, accent: Color32) {
        if self.settings_popup_open {
            let clicked_outside = ctx.input(|i| i.pointer.any_click())
                && !ctx.input(|i| i.pointer.interact_pos())
                    .map(|pos| {
                        if let Some(rect) = self.settings_popup_rect {
                            rect.contains(pos)
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
            if clicked_outside {
                self.settings_popup_open = false;
            }
        }

        egui::Area::new("settings_fab".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, Vec2::new(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                if self.settings_popup_open {
                    let frame_resp = egui::Frame::none()
                        .fill(surface_highest())
                        .rounding(Rounding::same(20.0))
                        .stroke(Stroke::new(1.0_f32, outline_var()))
                        .inner_margin(egui::Margin::same(16.0))
                        .show(ui, |ui| {
                            ui.set_width(260.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("SETTINGS").color(on_surface_var()).size(10.0).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let (resp, painter) = ui.allocate_painter(Vec2::splat(22.0), egui::Sense::click());
                                    let center = resp.rect.center();
                                    let col = if resp.hovered() { on_surface() } else { on_surface_var() };
                                    let r = 5.0_f32;
                                    painter.line_segment(
                                        [center + Vec2::new(-r, -r), center + Vec2::new(r, r)],
                                        Stroke::new(1.6_f32, col),
                                    );
                                    painter.line_segment(
                                        [center + Vec2::new(-r, r), center + Vec2::new(r, -r)],
                                        Stroke::new(1.6_f32, col),
                                    );
                                    if resp.clicked() {
                                        self.settings_popup_open = false;
                                    }
                                });
                            });
                            ui.add_space(10.0);

                            ui.label(egui::RichText::new("Accent color").color(on_surface_var()).size(12.0));
                            ui.add_space(6.0);
                            ui.horizontal_wrapped(|ui| {
                                let presets: &[([u8;3], &str)] = &[
                                    ([103, 80, 164], "Purple"),
                                    ([25, 118, 210], "Blue"),
                                    ([0, 150, 136], "Teal"),
                                    ([56, 142, 60], "Green"),
                                    ([239, 108, 0], "Orange"),
                                    ([194, 24, 91], "Pink"),
                                ];
                                for (color, name) in presets {
                                    let c = Color32::from_rgb(color[0], color[1], color[2]);
                                    let is_cur = self.settings.accent_color == *color;
                                    let size = Vec2::splat(if is_cur { 26.0 } else { 22.0 });
                                    let (resp, painter) = ui.allocate_painter(size, egui::Sense::click());
                                    let rect = resp.rect;
                                    painter.rect_filled(rect, Rounding::same(if is_cur { 8.0 } else { 11.0 }), c);
                                    if is_cur {
                                        painter.rect_stroke(rect.shrink(2.0), Rounding::same(6.0), Stroke::new(2.0_f32, Color32::WHITE));
                                    }
                                    if resp.on_hover_text(*name).clicked() {
                                        self.settings.accent_color = *color;
                                        self.color_edit = [
                                            color[0] as f32 / 255.0,
                                            color[1] as f32 / 255.0,
                                            color[2] as f32 / 255.0,
                                        ];
                                        save_settings(&self.settings);
                                    }
                                    ui.add_space(3.0);
                                }
                            });
                            ui.add_space(6.0);
                            if ui.color_edit_button_rgb(&mut self.color_edit).changed() {
                                self.settings.accent_color = [
                                    (self.color_edit[0] * 255.0) as u8,
                                    (self.color_edit[1] * 255.0) as u8,
                                    (self.color_edit[2] * 255.0) as u8,
                                ];
                                save_settings(&self.settings);
                            }

                            ui.add_space(14.0);
                            ui.separator();
                            ui.add_space(10.0);

                            ui.label(egui::RichText::new("Keybinds").color(on_surface_var()).size(12.0));
                            ui.add_space(6.0);

                            self.show_bind_row(ui, accent, BindAction::ToggleStartStop);
                            ui.label(
                                egui::RichText::new("Playing -> pause in place. Stopped -> resume from where it was paused.")
                                    .color(on_surface_var()).size(10.0)
                            );
                        });
                    self.settings_popup_rect = Some(frame_resp.response.rect);
                } else {
                    self.settings_popup_rect = None;
                    let size = 56.0_f32;
                    let (resp, painter) = ui.allocate_painter(Vec2::splat(size), egui::Sense::click());
                    let center = resp.rect.center();
                    let bg = if resp.hovered() {
                        Color32::from_rgb(
                            (accent.r() as u16 * 92 / 100) as u8,
                            (accent.g() as u16 * 92 / 100) as u8,
                            (accent.b() as u16 * 92 / 100) as u8,
                        )
                    } else {
                        accent
                    };
                    painter.rect_filled(
                        resp.rect,
                        Rounding::same(16.0),
                        bg,
                    );
                    painter.text(
                        center,
                        egui::Align2::CENTER_CENTER,
                        "⚙",
                        FontId::proportional(22.0),
                        Color32::WHITE,
                    );
                    if resp.on_hover_text("Settings").clicked() {
                        self.settings_popup_open = true;
                    }
                }
            });
    }

    fn show_bind_row(&mut self, ui: &mut egui::Ui, accent: Color32, action: BindAction) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(action.label()).color(on_surface_var()).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let is_capturing = self.bind_capture.as_ref().map(|c| c.action) == Some(action);
                let label = if is_capturing {
                    "Press a key...".to_string()
                } else {
                    self.settings.bind_for(action).map(vk_name).unwrap_or_else(|| "Not set".into())
                };
                let btn_col = if is_capturing { accent } else { surface_high() };
                let btn = egui::Button::new(egui::RichText::new(label).size(12.0).color(if is_capturing { Color32::WHITE } else { on_surface() }))
                    .fill(btn_col)
                    .stroke(Stroke::new(1.0, if is_capturing { Color32::TRANSPARENT } else { outline() }))
                    .rounding(Rounding::same(8.0))
                    .min_size(Vec2::new(90.0, 28.0));
                if ui.add(btn).clicked() {
                    self.bind_capture = Some(BindCapture { action });
                }
                if self.settings.bind_for(action).is_some() {
                    if ui.add(
                        egui::Button::new(egui::RichText::new("✖").size(12.0).color(on_surface_var()))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE)
                            .min_size(Vec2::splat(20.0))
                    ).on_hover_text("Clear bind").clicked() {
                        self.settings.clear_bind(action);
                        self.sync_hotkey_settings();
                        save_settings(&self.settings);
                    }
                }
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    let icon_data = egui::IconData {
        rgba: build_piano_rgba(64),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Easy Midi")
            .with_inner_size([900.0, 600.0])
            .with_min_inner_size([700.0, 450.0])
            .with_icon(icon_data),
        ..Default::default()
    };

    eframe::run_native(
        "Easy Midi",
        options,
        Box::new(|cc| {
            let fonts = egui::FontDefinitions::default();
            egui_extras::install_image_loaders(&cc.egui_ctx);
            cc.egui_ctx.set_fonts(fonts);
            Box::new(EasyMidi::new(cc))
        }),
    )
}