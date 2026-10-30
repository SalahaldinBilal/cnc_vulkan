//! Discord Rich Presence. Discord recognizes the game by the Steam folder name, so the EA app's
//! install never shows. We talk to the Discord client ourselves over its local pipe, under the
//! game's own Discord application, with what's being played. The game is read on the main
//! thread every couple of seconds, and a thread of ours does the talking. See
//! knowledge-base/discord.md.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{game, log};

/// Discord's own applications for the games, from its list of detectable games.
const ZERO_HOUR_APP: &str = "1215962340441198682";
const GENERALS_APP: &str = "1216521049978699857";

struct Addresses {
    the_game_logic: usize,
    game_mode: usize,
    /// Set while a map loads (Generals' m_loadingScene).
    loading_map: usize,
    the_player_list: usize,
}

static ZERO_HOUR: Addresses =
    Addresses { the_game_logic: 0x00A3_9B0C, game_mode: 0x94, loading_map: 0x51, the_player_list: 0x00A3_95A0 };
static GENERALS: Addresses =
    Addresses { the_game_logic: 0x00A6_BE84, game_mode: 0x98, loading_map: 0x51, the_player_list: 0x00A6_B968 };

/// GlobalData: the map's path, an AsciiString.
const GD_MAP_NAME: usize = 0x8;
const LOCAL_PLAYER: usize = 0xC;
/// Player: the side of its player template, an AsciiString.
const PLAYER_SIDE: usize = 0x28;
/// An AsciiString points at a reference count and a size, with the characters after them.
const ASCII_STRING_CHARS: usize = 4;

mod mode {
    pub const CAMPAIGN: i32 = 0;
    pub const LAN: i32 = 1;
    pub const SKIRMISH: i32 = 2;
    pub const REPLAY: i32 = 3;
    pub const SHELL: i32 = 4;
    pub const INTERNET: i32 = 5;
}

const SAMPLE_EVERY: Duration = Duration::from_secs(2);
/// Discord allows five updates in 20 seconds.
const MIN_SEND_GAP: Duration = Duration::from_secs(5);
const RECONNECT_EVERY: Duration = Duration::from_secs(10);

#[derive(Clone, PartialEq, Default)]
struct Activity {
    details: String,
    state: String,
    start: u64,
}

static ENABLED: AtomicBool = AtomicBool::new(true);
static WANTED: Mutex<Option<Activity>> = Mutex::new(None);
static LAST_SAMPLE: Mutex<Option<Instant>> = Mutex::new(None);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn install(game_id: &str) {
    let app = if game_id == "generals" { GENERALS_APP } else { ZERO_HOUR_APP };
    let _ = std::thread::Builder::new().name("discord".into()).spawn(move || run(app));
}

fn read<T: Copy>(at: usize) -> T {
    unsafe { *(at as *const T) }
}

fn ascii_string(data: usize) -> String {
    if data == 0 {
        return String::new();
    }
    let chars = unsafe { std::ffi::CStr::from_ptr((data + ASCII_STRING_CHARS) as *const std::ffi::c_char) };
    chars.to_string_lossy().into_owned()
}

/// Called every frame on the game's thread.
pub fn on_frame() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    {
        let mut last = LAST_SAMPLE.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < SAMPLE_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let (details, state) = describe();
    let mut wanted = WANTED.lock().unwrap();
    let start = match wanted.as_ref() {
        Some(a) if a.details == details => a.start,
        _ => SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    };
    *wanted = Some(Activity { details, state, start });
}

/// What's being played, as Discord's two lines. A build we don't know shows just the game.
fn describe() -> (String, String) {
    let (Some(build), Some(a)) = (game::build(), game::pick(&ZERO_HOUR, Some(&GENERALS))) else {
        return (String::new(), String::new());
    };
    let logic = read::<usize>(a.the_game_logic);
    let gd = read::<usize>(build.global_data);
    if logic == 0 || gd == 0 {
        return ("In the menus".into(), String::new());
    }
    let map = map_label(&ascii_string(read::<usize>(gd + GD_MAP_NAME)));
    let game_mode = read::<i32>(logic + a.game_mode);
    if read::<u8>(logic + a.loading_map) != 0 && game_mode != mode::SHELL {
        return (format!("Loading {map}"), String::new());
    }
    let details = match game_mode {
        mode::CAMPAIGN => campaign_label(&map),
        mode::LAN => format!("LAN game on {map}"),
        mode::SKIRMISH => format!("Skirmish on {map}"),
        mode::REPLAY => return (format!("Watching a replay on {map}"), String::new()),
        mode::INTERNET => format!("Online game on {map}"),
        _ => return ("In the menus".into(), String::new()),
    };
    let players = read::<usize>(a.the_player_list);
    let local = if players == 0 { 0 } else { read::<usize>(players + LOCAL_PLAYER) };
    let state = if local == 0 { String::new() } else { faction_label(&ascii_string(read::<usize>(local + PLAYER_SIDE))) };
    (details, state)
}

/// "Maps\Alpine Assault\Alpine Assault.map" is "Alpine Assault".
fn map_label(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.strip_suffix(".map").or_else(|| file.strip_suffix(".MAP")).unwrap_or(file).to_string()
}

/// The general each Generals Challenge map pits you against (Campaign.ini's GeneralName).
const CHALLENGE_OPPONENTS: [(&str, &str); 8] = [
    ("GC_AIRGENERAL", "General Granger"),
    ("GC_CHEMGENERAL", "Dr. Thrax"),
    ("GC_CHINABOSS", "General Leang"),
    ("GC_LASERGENERAL", "General Townes"),
    ("GC_NUKEGENERAL", "General Tao"),
    ("GC_STEALTH", "Prince Kassad"),
    ("GC_SUPERWEAPONSGENERAL", "General Alexander"),
    ("GC_TANKGENERAL", "General Kwai"),
];

/// Campaign maps are named by side and number (MD_GLA03, Zero Hour's with MD_ in front), with
/// a suffix for the cutscene maps around a mission (MD_GLA03_CINE, _INTRO, _END).
fn campaign_label(map: &str) -> String {
    let upper = map.to_ascii_uppercase();
    let name = upper.strip_prefix("MD_").unwrap_or(&upper);
    if let Some((_, opponent)) = CHALLENGE_OPPONENTS.iter().find(|(m, _)| *m == name) {
        return format!("Generals Challenge vs {opponent}");
    }
    if name.starts_with("GC_") {
        return "Generals Challenge".into();
    }
    if name.starts_with("TRAINING") {
        return "Training".into();
    }
    for (prefix, side) in [("USA", "USA"), ("CHI", "China"), ("GLA", "GLA")] {
        let Some(rest) = name.strip_prefix(prefix) else { continue };
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if let Ok(number) = rest[..digits].parse::<u32>() {
            let cutscene = if rest.len() > digits { " (cutscene)" } else { "" };
            return format!("{side} campaign, mission {number}{cutscene}");
        }
    }
    format!("Campaign: {map}")
}

#[cfg(test)]
mod tests {
    #[test]
    fn campaign_labels() {
        let label = |path: &str| super::campaign_label(&super::map_label(path));
        assert_eq!(label(r"maps\md_gla03\md_gla03.map"), "GLA campaign, mission 3");
        assert_eq!(label(r"Maps\MD_CHI05_INTRO\MD_CHI05_Intro.map"), "China campaign, mission 5 (cutscene)");
        assert_eq!(label(r"Maps\USA01\USA01.map"), "USA campaign, mission 1");
        assert_eq!(label(r"Maps\GC_ChemGeneral\GC_ChemGeneral.map"), "Generals Challenge vs Dr. Thrax");
        assert_eq!(label(r"Maps\Training01\Training01.map"), "Training");
    }
}

fn faction_label(side: &str) -> String {
    let name = match side {
        "" | "Civilian" | "Observer" | "ReplayObserver" => return String::new(),
        "America" | "USA" => "USA",
        "AmericaAirForceGeneral" => "USA Air Force General",
        "AmericaLaserGeneral" => "USA Laser General",
        "AmericaSuperWeaponGeneral" => "USA Superweapon General",
        "China" => "China",
        "ChinaTankGeneral" => "China Tank General",
        "ChinaInfantryGeneral" => "China Infantry General",
        "ChinaNukeGeneral" => "China Nuke General",
        "GLA" => "GLA",
        "GLAToxinGeneral" => "GLA Toxin General",
        "GLADemolitionGeneral" => "GLA Demolition General",
        "GLAStealthGeneral" => "GLA Stealth General",
        other => other,
    };
    format!("Playing as {name}")
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct Connection {
    pipe: File,
    nonce: u64,
}

impl Connection {
    fn open(app: &str) -> Option<Connection> {
        let pipe = (0..10).find_map(|i| File::options().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok())?;
        let mut c = Connection { pipe, nonce: 0 };
        c.send(0, &format!(r#"{{"v":1,"client_id":{}}}"#, json_string(app))).ok()?;
        let (op, _) = c.receive().ok()?;
        (op == 1).then_some(c)
    }

    fn send(&mut self, op: u32, json: &str) -> std::io::Result<()> {
        let mut frame = Vec::with_capacity(8 + json.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
        frame.extend_from_slice(json.as_bytes());
        self.pipe.write_all(&frame)
    }

    fn receive(&mut self) -> std::io::Result<(u32, Vec<u8>)> {
        let mut header = [0u8; 8];
        self.pipe.read_exact(&mut header)?;
        let op = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let mut body = vec![0u8; len];
        self.pipe.read_exact(&mut body)?;
        Ok((op, body))
    }

    /// Sets the activity, or clears it with None.
    fn set_activity(&mut self, activity: Option<&Activity>) -> std::io::Result<()> {
        self.nonce += 1;
        let body = match activity {
            None => "null".to_string(),
            Some(a) => {
                let mut fields = Vec::new();
                for (key, value) in [("details", &a.details), ("state", &a.state)] {
                    if !value.is_empty() {
                        fields.push(format!(r#""{key}":{}"#, json_string(value)));
                    }
                }
                fields.push(format!(r#""timestamps":{{"start":{}}}"#, a.start));
                format!("{{{}}}", fields.join(","))
            }
        };
        let json = format!(r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{body}}},"nonce":"{}"}}"#, std::process::id(), self.nonce);
        self.send(1, &json)?;
        self.receive().map(|_| ())
    }
}

fn run(app: &'static str) {
    let mut connection: Option<Connection> = None;
    let mut last_attempt: Option<Instant> = None;
    let mut sent: Option<Option<Activity>> = None;
    let mut last_send = Instant::now() - MIN_SEND_GAP;
    let mut logged = false;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let wanted = if ENABLED.load(Ordering::Relaxed) { WANTED.lock().unwrap().clone() } else { None };
        if connection.is_none() {
            if wanted.is_none() || last_attempt.is_some_and(|t| t.elapsed() < RECONNECT_EVERY) {
                continue;
            }
            last_attempt = Some(Instant::now());
            connection = Connection::open(app);
            sent = None;
            if connection.is_some() && !logged {
                log!("connected to Discord for the game's status");
                logged = true;
            }
        }
        let Some(c) = connection.as_mut() else { continue };
        if sent.as_ref() == Some(&wanted) || last_send.elapsed() < MIN_SEND_GAP {
            continue;
        }
        last_send = Instant::now();
        if c.set_activity(wanted.as_ref()).is_ok() {
            sent = Some(wanted);
        } else {
            connection = None;
        }
    }
}
