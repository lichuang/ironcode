//! Plan file management for plan mode.
//!
//! Plans are stored as Markdown files in the data directory's `plans/` folder.
//! The file name is derived from a hero slug that is stable for a given
//! `plan_session_id`, mirroring kimi-cli's `tools/plan/heroes.py`.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rand::seq::SliceRandom;

use crate::config::Config;
use crate::config::loader::{data_dir, default_data_dir};

/// Hero names used to generate readable plan file slugs.
const HERO_NAMES: &[&str] = &[
  "iron-man",
  "spider-man",
  "captain-america",
  "thor",
  "hulk",
  "black-widow",
  "hawkeye",
  "black-panther",
  "doctor-strange",
  "scarlet-witch",
  "vision",
  "falcon",
  "war-machine",
  "ant-man",
  "wasp",
  "captain-marvel",
  "gamora",
  "star-lord",
  "groot",
  "rocket",
  "drax",
  "mantis",
  "nebula",
  "shang-chi",
  "moon-knight",
  "ms-marvel",
  "she-hulk",
  "echo",
  "wolverine",
  "cyclops",
  "storm",
  "jean-grey",
  "rogue",
  "beast",
  "nightcrawler",
  "colossus",
  "shadowcat",
  "jubilee",
  "cable",
  "deadpool",
  "bishop",
  "magik",
  "iceman",
  "archangel",
  "psylocke",
  "dazzler",
  "forge",
  "havok",
  "polaris",
  "emma-frost",
  "namor",
  "silver-surfer",
  "adam-warlock",
  "nova",
  "quasar",
  "sentry",
  "blue-marvel",
  "spectrum",
  "squirrel-girl",
  "cloak",
  "dagger",
  "punisher",
  "elektra",
  "luke-cage",
  "iron-fist",
  "jessica-jones",
  "daredevil",
  "blade",
  "ghost-rider",
  "morbius",
  "venom",
  "carnage",
  "silk",
  "spider-gwen",
  "miles-morales",
  "america-chavez",
  "kate-bishop",
  "yelena-belova",
  "white-tiger",
  "moon-girl",
  "devil-dinosaur",
  "amadeus-cho",
  "riri-williams",
  "kamala-khan",
  "sam-alexander",
  "nova-prime",
  "medusa",
  "black-bolt",
  "crystal",
  "karnak",
  "gorgon",
  "lockjaw",
  "quake",
  "mockingbird",
  "bobbi-morse",
  "maria-hill",
  "nick-fury",
  "phil-coulson",
  "winter-soldier",
  "us-agent",
  "patriot",
  "speed",
  "wiccan",
  "hulkling",
  "stature",
  "yellowjacket",
  "tigra",
  "hellcat",
  "valkyrie",
  "sif",
  "beta-ray-bill",
  "hercules",
  "wonder-man",
  "taskmaster",
  "domino",
  "cannonball",
  "sunspot",
  "wolfsbane",
  "warpath",
  "multiple-man",
  "banshee",
  "siryn",
  "monet",
  "rictor",
  "shatterstar",
  "longshot",
  "daken",
  "x-23",
  "fantomex",
  "batman",
  "superman",
  "wonder-woman",
  "flash",
  "aquaman",
  "green-lantern",
  "martian-manhunter",
  "cyborg",
  "hawkgirl",
  "green-arrow",
  "black-canary",
  "zatanna",
  "constantine",
  "shazam",
  "blue-beetle",
  "booster-gold",
  "firestorm",
  "atom",
  "hawkman",
  "plastic-man",
  "red-tornado",
  "starfire",
  "raven",
  "beast-boy",
  "robin",
  "nightwing",
  "batgirl",
  "batwoman",
  "red-hood",
  "signal",
  "orphan",
  "spoiler",
  "catwoman",
  "huntress",
  "supergirl",
  "superboy",
  "power-girl",
  "steel",
  "stargirl",
  "doctor-fate",
  "mister-terrific",
  "hourman",
  "sandman",
  "spectre",
  "phantom-stranger",
  "swamp-thing",
  "animal-man",
  "deadman",
  "vixen",
  "black-lightning",
  "static",
  "icon",
  "rocket-dc",
  "captain-atom",
  "fire",
  "ice",
  "elongated-man",
  "metamorpho",
  "black-hawk",
  "crimson-avenger",
  "doctor-mid-nite",
  "jakeem-thunder",
  "mister-miracle",
  "big-barda",
  "orion",
  "lightray",
  "forager",
  "killer-frost",
  "jessica-cruz",
  "simon-baz",
  "john-stewart",
  "guy-gardner",
  "kyle-rayner",
  "hal-jordan",
  "wally-west",
  "barry-allen",
  "jay-garrick",
  "impulse",
  "kid-flash",
  "donna-troy",
  "tempest",
  "aqualad",
  "miss-martian",
  "terra",
  "jericho",
  "ravager",
  "red-star",
  "pantha",
  "argent",
  "damage",
  "jade",
  "obsidian",
  "cyclone",
  "atom-smasher",
  "maxima",
  "starman",
  "liberty-belle",
];

fn slug_cache() -> &'static Mutex<HashMap<String, String>> {
  static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
  CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Pre-warm the in-process slug cache with a previously persisted slug.
pub fn seed_slug_cache(session_id: &str, slug: &str) {
  slug_cache()
    .lock()
    .expect("slug cache lock")
    .insert(session_id.to_string(), slug.to_string());
}

/// Get or create a hero slug for the given plan session id.
pub fn get_or_create_slug(session_id: &str) -> String {
  {
    let cache = slug_cache().lock().expect("slug cache lock");
    if let Some(slug) = cache.get(session_id) {
      return slug.clone();
    }
  }

  let plans_dir = plans_dir(None);
  if !plans_dir.exists() {
    let _ = fs::create_dir_all(&plans_dir);
  }

  let mut rng = rand::thread_rng();
  let mut slug = String::new();
  for _ in 0..20 {
    let words: Vec<_> = (0..3)
      .map(|_| *HERO_NAMES.choose(&mut rng).expect("hero names non-empty"))
      .collect();
    slug = words.join("-");
    if !plans_dir.join(format!("{slug}.md")).exists() {
      break;
    }
  }

  slug_cache()
    .lock()
    .expect("slug cache lock")
    .insert(session_id.to_string(), slug.clone());
  slug
}

/// Get the plans directory path.
///
/// If `config` is provided, uses `data_dir(config)`; otherwise falls back to
/// `~/.ironcode/plans`.
pub fn plans_dir(config: Option<&Config>) -> PathBuf {
  let base = config
    .map(data_dir)
    .or_else(default_data_dir)
    .unwrap_or_else(|| PathBuf::from(".ironcode"));
  base.join("plans")
}

/// Get the plan file path for a given plan session id.
pub fn plan_file_path(plan_session_id: &str, config: Option<&Config>) -> PathBuf {
  plans_dir(config).join(format!("{}.md", get_or_create_slug(plan_session_id)))
}

/// Read the plan file for a plan session.
///
/// Returns `None` if the file does not exist or cannot be read.
pub fn read_plan(plan_session_id: &str, config: Option<&Config>) -> Option<String> {
  let path = plan_file_path(plan_session_id, config);
  fs::read_to_string(&path).ok()
}

/// Write content to the plan file for a plan session.
///
/// Creates the `plans/` directory if it does not exist.
#[allow(dead_code)]
pub fn write_plan(
  plan_session_id: &str,
  content: &str,
  config: Option<&Config>,
) -> std::io::Result<()> {
  let dir = plans_dir(config);
  if !dir.exists() {
    fs::create_dir_all(&dir)?;
  }
  let path = plan_file_path(plan_session_id, config);
  let mut file = fs::OpenOptions::new()
    .create(true)
    .truncate(true)
    .write(true)
    .open(&path)?;
  file.write_all(content.as_bytes())?;
  file.flush()
}

/// Check whether a plan file exists for a plan session.
#[allow(dead_code)]
pub fn plan_exists(plan_session_id: &str, config: Option<&Config>) -> bool {
  plan_file_path(plan_session_id, config).exists()
}

/// Delete the plan file for a plan session.
///
/// Returns `true` if a file was deleted, `false` if it did not exist.
#[allow(dead_code)]
pub fn delete_plan(plan_session_id: &str, config: Option<&Config>) -> std::io::Result<bool> {
  let path = plan_file_path(plan_session_id, config);
  if path.exists() {
    fs::remove_file(&path)?;
    Ok(true)
  } else {
    Ok(false)
  }
}
