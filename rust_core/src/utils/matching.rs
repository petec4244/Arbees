use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use strsim::jaro_winkler;

/// Type alias for the team aliases cache structure
type AliasMap = HashMap<&'static str, Vec<&'static str>>;
type SportAliasCache = HashMap<&'static str, AliasMap>;

/// Global cache for team aliases, initialized once on first access
static TEAM_ALIASES_CACHE: OnceLock<SportAliasCache> = OnceLock::new();

/// Initialize the global team aliases cache with all sports
fn init_team_aliases_cache() -> SportAliasCache {
    let mut cache = HashMap::new();

    // Pre-populate cache for all supported sports
    for sport in &["nba", "nfl", "nhl", "mlb", "ncaab", "ncaaf", "soccer", "mma", "tennis"] {
        cache.insert(*sport, build_team_aliases(sport));
    }

    cache
}

/// Match confidence level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchConfidence {
    None = 0,
    Low = 1,    // Fuzzy match only - risky
    Medium = 2, // Partial alias or word match
    High = 3,   // Strong alias match or multiple words
    Exact = 4,  // Normalized exact match
}

/// Result of matching two team names
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub confidence: MatchConfidence,
    pub score: f64,
    pub reason: String,
}

impl MatchResult {
    fn none() -> Self {
        Self {
            confidence: MatchConfidence::None,
            score: 0.0,
            reason: "No match".to_string(),
        }
    }

    fn exact() -> Self {
        Self {
            confidence: MatchConfidence::Exact,
            score: 1.0,
            reason: "Exact match".to_string(),
        }
    }

    fn high(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::High,
            score,
            reason: reason.to_string(),
        }
    }

    fn medium(score: f64, reason: &str) -> Self {
        Self {
            confidence: MatchConfidence::Medium,
            score,
            reason: reason.to_string(),
        }
    }

    pub fn is_match(&self) -> bool {
        self.confidence >= MatchConfidence::Medium
    }
}

// =============================================================================
// CONTEXT-BASED MATCHING TYPES
// =============================================================================

/// Sport-specific scoring characteristics for validation tolerances
#[derive(Debug, Clone)]
pub struct SportScoring {
    /// Typical total score for a game (e.g., NBA ~220, NHL ~6)
    pub typical_total: f64,
    /// Minimum margin to be meaningful (e.g., 2 goals in NHL matters more than 2 pts in NBA)
    pub meaningful_margin: u32,
    /// How volatile scores are (0.0-1.0, higher = more variance)
    pub score_volatility: f64,
}

impl Default for SportScoring {
    fn default() -> Self {
        Self {
            typical_total: 100.0,
            meaningful_margin: 5,
            score_volatility: 0.5,
        }
    }
}

/// Game context for enhanced matching - provides current game state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameContext {
    pub home_team: String,
    pub away_team: String,
    pub home_score: Option<u32>,
    pub away_score: Option<u32>,
    /// Current period/quarter/half (e.g., "Q2", "3rd", "2H")
    pub period: Option<String>,
    /// Time remaining in period (e.g., "5:32", "12:00")
    pub time_remaining: Option<String>,
    /// Sport identifier (e.g., "nba", "nhl", "nfl")
    pub sport: String,
}

/// Market context for enhanced matching - provides market metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MarketContext {
    /// The market title/question (e.g., "Lakers vs Celtics: Who will win?")
    pub market_title: Option<String>,
    /// Sport tag from the market platform
    pub market_sport: Option<String>,
    /// Team names extracted from the market (for opponent validation)
    pub market_participants: Vec<String>,
}

/// Enhanced match result with context validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMatchResult {
    /// The underlying name-based match result
    pub name_match: NameMatchSummary,
    /// Whether the sport from game context matches market context
    pub sport_valid: bool,
    /// Score for opponent validation (0.0-1.0, 1.0 = opponent found in market)
    pub opponent_score: f64,
    /// Score correlation if scores extractable from market (0.0-1.0 or None)
    pub score_correlation: Option<f64>,
    /// Final combined confidence score (0.0-1.0)
    pub final_confidence: f64,
    /// If rejected, the reason why
    pub rejection_reason: Option<String>,
}

/// Serializable summary of MatchResult for RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameMatchSummary {
    pub confidence_level: String,
    pub score: f64,
    pub reason: String,
    pub is_match: bool,
}

impl From<&MatchResult> for NameMatchSummary {
    fn from(result: &MatchResult) -> Self {
        Self {
            confidence_level: format!("{:?}", result.confidence),
            score: result.score,
            reason: result.reason.clone(),
            is_match: result.is_match(),
        }
    }
}

/// Get team aliases for a sport from the cached global map.
/// Returns a reference to the cached map for the given sport.
/// All names are lowercase.
fn get_team_aliases(sport: &str) -> &'static AliasMap {
    static EMPTY_MAP: OnceLock<AliasMap> = OnceLock::new();

    let cache = TEAM_ALIASES_CACHE.get_or_init(init_team_aliases_cache);

    // Normalize sport name for lookup
    let sport_lower = sport.to_lowercase();
    let sport_key = match sport_lower.as_str() {
        "nba" => "nba",
        "nfl" => "nfl",
        "nhl" => "nhl",
        "mlb" => "mlb",
        "ncaab" | "ncaaf" => "ncaab", // Share college aliases
        "soccer" | "mls" => "soccer",
        "mma" | "ufc" => "mma",
        "tennis" | "atp" | "wta" => "tennis",
        _ => return EMPTY_MAP.get_or_init(HashMap::new),
    };

    cache.get(sport_key).unwrap_or_else(|| EMPTY_MAP.get_or_init(HashMap::new))
}

/// Build team aliases for a sport. Called once during cache initialization.
/// All names should be lowercase.
fn build_team_aliases(sport: &str) -> AliasMap {
    let mut map: AliasMap = HashMap::new();

    match sport {
        "nba" => {
            // Format: canonical_name -> [aliases including city, abbr, nicknames]
            map.insert(
                "76ers",
                vec![
                    "sixers",
                    "philadelphia 76ers",
                    "philadelphia sixers",
                    "phi",
                    "philly",
                ],
            );
            map.insert("bucks", vec!["milwaukee bucks", "milwaukee", "mil"]);
            map.insert("bulls", vec!["chicago bulls", "chicago", "chi"]);
            map.insert(
                "cavaliers",
                vec!["cavs", "cleveland cavaliers", "cleveland", "cle"],
            );
            map.insert("celtics", vec!["boston celtics", "boston", "bos"]);
            map.insert(
                "clippers",
                vec!["la clippers", "los angeles clippers", "clips", "lac"],
            );
            map.insert("grizzlies", vec!["memphis grizzlies", "memphis", "mem"]);
            map.insert("hawks", vec!["atlanta hawks", "atlanta", "atl"]);
            map.insert("heat", vec!["miami heat", "miami", "mia"]);
            map.insert("hornets", vec!["charlotte hornets", "charlotte", "cha"]);
            map.insert("jazz", vec!["utah jazz", "utah", "uta"]);
            map.insert("kings", vec!["sacramento kings", "sacramento", "sac"]);
            map.insert("knicks", vec!["new york knicks", "ny knicks", "nyk"]);
            map.insert("lakers", vec!["la lakers", "los angeles lakers", "lal"]);
            map.insert("magic", vec!["orlando magic", "orlando", "orl"]);
            map.insert(
                "mavericks",
                vec!["mavs", "dallas mavericks", "dallas", "dal"],
            );
            map.insert("nets", vec!["brooklyn nets", "brooklyn", "bkn"]);
            map.insert("nuggets", vec!["denver nuggets", "denver", "den"]);
            map.insert("pacers", vec!["indiana pacers", "indiana", "ind"]);
            map.insert(
                "pelicans",
                vec!["pels", "new orleans pelicans", "new orleans", "nop"],
            );
            map.insert("pistons", vec!["detroit pistons", "detroit", "det"]);
            map.insert("raptors", vec!["toronto raptors", "toronto", "tor"]);
            map.insert("rockets", vec!["houston rockets", "houston", "hou"]);
            map.insert("spurs", vec!["san antonio spurs", "san antonio", "sas"]);
            map.insert("suns", vec!["phoenix suns", "phoenix", "phx"]);
            map.insert(
                "thunder",
                vec!["oklahoma city thunder", "oklahoma city", "okc"],
            );
            map.insert(
                "timberwolves",
                vec!["wolves", "minnesota timberwolves", "minnesota", "min"],
            );
            map.insert(
                "trail blazers",
                vec!["blazers", "portland trail blazers", "portland", "por"],
            );
            map.insert(
                "warriors",
                vec!["golden state warriors", "golden state", "gsw", "gs", "dubs"],
            );
            map.insert("wizards", vec!["washington wizards", "washington", "was"]);
        }
        "nfl" => {
            map.insert(
                "49ers",
                vec!["niners", "san francisco 49ers", "san francisco", "sf"],
            );
            map.insert("bears", vec!["chicago bears", "chicago", "chi"]);
            map.insert("bengals", vec!["cincinnati bengals", "cincinnati", "cin"]);
            map.insert("bills", vec!["buffalo bills", "buffalo", "buf"]);
            map.insert("broncos", vec!["denver broncos", "denver", "den"]);
            map.insert("browns", vec!["cleveland browns", "cleveland", "cle"]);
            map.insert(
                "buccaneers",
                vec!["bucs", "tampa bay buccaneers", "tampa bay", "tampa", "tb"],
            );
            map.insert(
                "cardinals",
                vec!["cards", "arizona cardinals", "arizona", "ari"],
            );
            map.insert(
                "chargers",
                vec!["la chargers", "los angeles chargers", "lac"],
            );
            map.insert("chiefs", vec!["kansas city chiefs", "kansas city", "kc"]);
            map.insert("colts", vec!["indianapolis colts", "indianapolis", "ind"]);
            map.insert(
                "commanders",
                vec!["washington commanders", "washington", "was"],
            );
            map.insert("cowboys", vec!["dallas cowboys", "dallas", "dal"]);
            map.insert("dolphins", vec!["miami dolphins", "miami", "mia"]);
            map.insert(
                "eagles",
                vec!["philadelphia eagles", "philadelphia", "phi", "philly"],
            );
            map.insert("falcons", vec!["atlanta falcons", "atlanta", "atl"]);
            map.insert("giants", vec!["new york giants", "ny giants", "nyg"]);
            map.insert(
                "jaguars",
                vec!["jags", "jacksonville jaguars", "jacksonville", "jax"],
            );
            map.insert("jets", vec!["new york jets", "ny jets", "nyj"]);
            map.insert("lions", vec!["detroit lions", "detroit", "det"]);
            map.insert("packers", vec!["green bay packers", "green bay", "gb"]);
            map.insert("panthers", vec!["carolina panthers", "carolina", "car"]);
            map.insert(
                "patriots",
                vec!["pats", "new england patriots", "new england", "ne"],
            );
            map.insert("raiders", vec!["las vegas raiders", "las vegas", "lv"]);
            map.insert("rams", vec!["la rams", "los angeles rams", "lar"]);
            map.insert("ravens", vec!["baltimore ravens", "baltimore", "bal"]);
            map.insert("saints", vec!["new orleans saints", "new orleans", "no"]);
            map.insert("seahawks", vec!["seattle seahawks", "seattle", "sea"]);
            map.insert("steelers", vec!["pittsburgh steelers", "pittsburgh", "pit"]);
            map.insert("texans", vec!["houston texans", "houston", "hou"]);
            map.insert("titans", vec!["tennessee titans", "tennessee", "ten"]);
            map.insert("vikings", vec!["minnesota vikings", "minnesota", "min"]);
        }
        "nhl" => {
            map.insert(
                "avalanche",
                vec!["avs", "colorado avalanche", "colorado", "col"],
            );
            map.insert("blackhawks", vec!["chicago blackhawks", "chicago", "chi"]);
            map.insert(
                "blue jackets",
                vec!["jackets", "columbus blue jackets", "columbus", "cbj"],
            );
            map.insert("blues", vec!["st louis blues", "st. louis blues", "stl"]);
            map.insert("bruins", vec!["boston bruins", "boston", "bos"]);
            map.insert(
                "canadiens",
                vec!["habs", "montreal canadiens", "montreal", "mtl"],
            );
            map.insert("canucks", vec!["vancouver canucks", "vancouver", "van"]);
            map.insert(
                "capitals",
                vec!["caps", "washington capitals", "washington", "wsh"],
            );
            map.insert(
                "coyotes",
                vec!["yotes", "arizona coyotes", "arizona", "ari"],
            );
            map.insert(
                "devils",
                vec!["new jersey devils", "new jersey", "nj", "njd"],
            );
            map.insert("ducks", vec!["anaheim ducks", "anaheim", "ana"]);
            map.insert("flames", vec!["calgary flames", "calgary", "cgy"]);
            map.insert("flyers", vec!["philadelphia flyers", "philadelphia", "phi"]);
            map.insert(
                "golden knights",
                vec!["knights", "vegas golden knights", "vegas", "vgk"],
            );
            map.insert(
                "hurricanes",
                vec!["canes", "carolina hurricanes", "carolina", "car"],
            );
            map.insert("islanders", vec!["isles", "new york islanders", "nyi"]);
            map.insert("jets", vec!["winnipeg jets", "winnipeg", "wpg"]);
            map.insert("kings", vec!["la kings", "los angeles kings", "lak"]);
            map.insert("kraken", vec!["seattle kraken", "seattle", "sea"]);
            map.insert(
                "lightning",
                vec!["bolts", "tampa bay lightning", "tampa bay", "tbl"],
            );
            map.insert(
                "maple leafs",
                vec!["leafs", "toronto maple leafs", "toronto", "tor"],
            );
            map.insert("oilers", vec!["edmonton oilers", "edmonton", "edm"]);
            map.insert("panthers", vec!["florida panthers", "florida", "fla"]);
            map.insert(
                "penguins",
                vec!["pens", "pittsburgh penguins", "pittsburgh", "pit"],
            );
            map.insert(
                "predators",
                vec!["preds", "nashville predators", "nashville", "nsh"],
            );
            map.insert("rangers", vec!["new york rangers", "nyr"]);
            map.insert(
                "red wings",
                vec!["wings", "detroit red wings", "detroit", "det"],
            );
            map.insert("sabres", vec!["buffalo sabres", "buffalo", "buf"]);
            map.insert("senators", vec!["sens", "ottawa senators", "ottawa", "ott"]);
            map.insert("sharks", vec!["san jose sharks", "san jose", "sjs"]);
            map.insert("stars", vec!["dallas stars", "dallas", "dal"]);
            map.insert("wild", vec!["minnesota wild", "minnesota", "min"]);
        }
        "mlb" => {
            map.insert(
                "angels",
                vec!["los angeles angels", "la angels", "anaheim angels", "laa"],
            );
            map.insert("astros", vec!["stros", "houston astros", "houston", "hou"]);
            map.insert(
                "athletics",
                vec!["a's", "as", "oakland athletics", "oakland", "oak"],
            );
            map.insert(
                "blue jays",
                vec!["jays", "toronto blue jays", "toronto", "tor"],
            );
            map.insert("braves", vec!["atlanta braves", "atlanta", "atl"]);
            map.insert("brewers", vec!["milwaukee brewers", "milwaukee", "mil"]);
            map.insert(
                "cardinals",
                vec!["cards", "st louis cardinals", "st. louis cardinals", "stl"],
            );
            map.insert("cubs", vec!["chicago cubs", "chi cubs", "chc"]);
            map.insert(
                "diamondbacks",
                vec![
                    "dbacks",
                    "d-backs",
                    "arizona diamondbacks",
                    "arizona",
                    "ari",
                ],
            );
            map.insert("dodgers", vec!["los angeles dodgers", "la dodgers", "lad"]);
            map.insert("giants", vec!["san francisco giants", "sf giants", "sf"]);
            map.insert("guardians", vec!["cleveland guardians", "cleveland", "cle"]);
            map.insert("mariners", vec!["seattle mariners", "seattle", "sea"]);
            map.insert(
                "marlins",
                vec!["miami marlins", "florida marlins", "miami", "mia"],
            );
            map.insert("mets", vec!["new york mets", "ny mets", "nym"]);
            map.insert(
                "nationals",
                vec!["nats", "washington nationals", "washington", "wsh"],
            );
            map.insert(
                "orioles",
                vec!["o's", "os", "baltimore orioles", "baltimore", "bal"],
            );
            map.insert("padres", vec!["san diego padres", "san diego", "sd"]);
            map.insert(
                "phillies",
                vec!["philadelphia phillies", "philadelphia", "phi", "philly"],
            );
            map.insert("pirates", vec!["pittsburgh pirates", "pittsburgh", "pit"]);
            map.insert("rangers", vec!["texas rangers", "texas", "tex"]);
            map.insert("rays", vec!["tampa bay rays", "tampa bay", "tb"]);
            map.insert("red sox", vec!["redsox", "boston red sox", "boston", "bos"]);
            map.insert("reds", vec!["cincinnati reds", "cincinnati", "cin"]);
            map.insert("rockies", vec!["colorado rockies", "colorado", "col"]);
            map.insert("royals", vec!["kansas city royals", "kansas city", "kc"]);
            map.insert("tigers", vec!["detroit tigers", "detroit", "det"]);
            map.insert("twins", vec!["minnesota twins", "minnesota", "min"]);
            map.insert(
                "white sox",
                vec!["whitesox", "chicago white sox", "chi sox", "chw"],
            );
            map.insert(
                "yankees",
                vec!["yanks", "new york yankees", "ny yankees", "nyy"],
            );
        }
        "ncaab" | "ncaaf" => {
            // College teams - include common nicknames and abbreviations
            map.insert("alabama", vec!["bama", "crimson tide", "ala"]);
            map.insert("arizona", vec!["wildcats", "ari", "zona"]);
            map.insert("arizona state", vec!["sun devils", "asu"]);
            map.insert("arkansas", vec!["razorbacks", "hogs", "ark"]);
            map.insert("auburn", vec!["tigers", "aub"]);
            map.insert("baylor", vec!["bears", "bay"]);
            map.insert("boston college", vec!["eagles", "bc"]);
            map.insert("clemson", vec!["tigers", "clem"]);
            map.insert("colorado", vec!["buffaloes", "buffs", "col", "cu"]);
            map.insert("connecticut", vec!["uconn", "huskies", "conn"]);
            map.insert("duke", vec!["blue devils"]);
            map.insert("florida", vec!["gators", "fla", "uf"]);
            map.insert("florida state", vec!["seminoles", "noles", "fsu"]);
            map.insert("georgia", vec!["bulldogs", "dawgs", "uga"]);
            map.insert("gonzaga", vec!["zags", "bulldogs"]);
            map.insert("houston", vec!["cougars", "coogs", "hou"]);
            map.insert("illinois", vec!["fighting illini", "illini", "ill"]);
            map.insert("indiana", vec!["hoosiers", "ind", "iu"]);
            map.insert("iowa", vec!["hawkeyes"]);
            map.insert("iowa state", vec!["cyclones", "isu"]);
            map.insert("kansas", vec!["jayhawks", "ku"]);
            map.insert("kansas state", vec!["wildcats", "ksu", "k-state"]);
            map.insert("kentucky", vec!["wildcats", "uk", "ky"]);
            map.insert("louisiana state", vec!["lsu", "tigers"]);
            map.insert("louisville", vec!["cardinals", "cards", "lou"]);
            map.insert("marquette", vec!["golden eagles", "marq"]);
            map.insert("maryland", vec!["terrapins", "terps", "md"]);
            map.insert("memphis", vec!["tigers", "mem"]);
            map.insert("miami", vec!["hurricanes", "canes", "mia"]);
            map.insert("michigan", vec!["wolverines", "mich", "um"]);
            map.insert("michigan state", vec!["spartans", "msu"]);
            map.insert("minnesota", vec!["golden gophers", "gophers", "minn"]);
            map.insert("mississippi", vec!["ole miss", "rebels"]);
            map.insert("mississippi state", vec!["bulldogs", "miss st", "msst"]);
            map.insert("missouri", vec!["tigers", "mizzou", "miz"]);
            map.insert("north carolina", vec!["tar heels", "unc", "carolina"]);
            map.insert("north carolina state", vec!["wolfpack", "nc state", "ncsu"]);
            map.insert("notre dame", vec!["fighting irish", "irish", "nd"]);
            map.insert("ohio state", vec!["buckeyes", "osu"]);
            map.insert("oklahoma", vec!["sooners", "ou"]);
            map.insert("oklahoma state", vec!["cowboys", "okst"]);
            map.insert("oregon", vec!["ducks", "ore"]);
            map.insert("oregon state", vec!["beavers", "orst"]);
            map.insert("penn state", vec!["nittany lions", "psu"]);
            map.insert("pittsburgh", vec!["panthers", "pitt"]);
            map.insert("purdue", vec!["boilermakers", "pur"]);
            map.insert("rutgers", vec!["scarlet knights", "rut"]);
            map.insert("san diego state", vec!["aztecs", "sdsu"]);
            map.insert("south carolina", vec!["gamecocks", "scar"]);
            map.insert("stanford", vec!["cardinal", "stan"]);
            map.insert("syracuse", vec!["orange", "cuse"]);
            map.insert("tcu", vec!["horned frogs"]);
            map.insert("tennessee", vec!["volunteers", "vols", "tenn"]);
            map.insert("texas", vec!["longhorns", "horns", "tex", "ut"]);
            map.insert("texas a&m", vec!["aggies", "tamu"]);
            map.insert("texas tech", vec!["red raiders", "ttu"]);
            map.insert("ucla", vec!["bruins"]);
            map.insert(
                "usc",
                vec!["trojans", "southern cal", "southern california"],
            );
            map.insert("utah", vec!["utes"]);
            map.insert("vanderbilt", vec!["commodores", "vandy"]);
            map.insert("villanova", vec!["wildcats", "nova"]);
            map.insert("virginia", vec!["cavaliers", "cavs", "uva", "hoos"]);
            map.insert("virginia tech", vec!["hokies", "vt"]);
            map.insert("wake forest", vec!["demon deacons", "wake"]);
            map.insert("washington", vec!["huskies", "udub", "uw"]);
            map.insert("west virginia", vec!["mountaineers", "wvu"]);
            map.insert("wisconsin", vec!["badgers", "wisc"]);
            map.insert("xavier", vec!["musketeers"]);
        }
        "soccer" => {
            // Premier League
            map.insert("arsenal", vec!["gunners", "ars"]);
            map.insert("aston villa", vec!["villa", "avl"]);
            map.insert("bournemouth", vec!["cherries", "bou"]);
            map.insert("brentford", vec!["bees", "bre"]);
            map.insert(
                "brighton",
                vec!["seagulls", "brighton and hove albion", "bha"],
            );
            map.insert("chelsea", vec!["blues", "che"]);
            map.insert("crystal palace", vec!["palace", "eagles", "cry"]);
            map.insert("everton", vec!["toffees", "eve"]);
            map.insert("fulham", vec!["cottagers", "ful"]);
            map.insert("ipswich", vec!["tractor boys", "ipswich town", "ips"]);
            map.insert("leicester", vec!["foxes", "leicester city", "lei"]);
            map.insert("liverpool", vec!["reds", "liv"]);
            map.insert("manchester city", vec!["man city", "city", "mci"]);
            map.insert(
                "manchester united",
                vec!["man utd", "man united", "united", "mun"],
            );
            map.insert("newcastle", vec!["magpies", "newcastle united", "new"]);
            map.insert("nottingham forest", vec!["forest", "nfo"]);
            map.insert("southampton", vec!["saints", "sou"]);
            map.insert("tottenham", vec!["spurs", "tottenham hotspur", "tot"]);
            map.insert("west ham", vec!["hammers", "west ham united", "whu"]);
            map.insert(
                "wolves",
                vec!["wolverhampton", "wolverhampton wanderers", "wol"],
            );
            // Top European clubs
            map.insert("barcelona", vec!["barca", "fcb"]);
            map.insert("real madrid", vec!["real", "madrid", "rma"]);
            map.insert("atletico madrid", vec!["atletico", "atm"]);
            map.insert("bayern munich", vec!["bayern", "fcb"]);
            map.insert("borussia dortmund", vec!["dortmund", "bvb"]);
            map.insert("paris saint germain", vec!["psg", "paris"]);
            map.insert("juventus", vec!["juve"]);
            map.insert("inter milan", vec!["inter", "internazionale"]);
            map.insert("ac milan", vec!["milan"]);
            map.insert("napoli", vec!["nap"]);
        }
        "mma" | "ufc" => {
            // MMA typically uses fighter names, not team names
            // Add common name variations if needed
        }
        "tennis" => {
            // Tennis uses player names
        }
        _ => {}
    }

    map
}

/// Words that are too generic to be reliable team identifiers
const GENERIC_WORDS: &[&str] = &[
    "state",
    "city",
    "university",
    "college",
    "fc",
    "united",
    "team",
    "the",
    "of",
    "and",
    "vs",
    "at",
    "in",
    "to",
    "for",
    "los",
    "san",
    "new",
    "las", // City prefixes
];

/// Words that indicate non-moneyline market types
const NON_MONEYLINE_INDICATORS: &[&str] = &[
    "over", "under", "o/u", "total", "spread", "handicap", "points", "goals", "runs", "score",
    "combined", "first to", "most", "mvp", "player", "quarter", "half", "period", "inning",
    "how many", "exact", "margin",
    // Time-sliced markets (1H/2H/quarters/periods)
    "1h", "2h", "1st half", "first half", "2nd half", "second half",
    "1q", "2q", "3q", "4q", "q1", "q2", "q3", "q4",
    "1st quarter", "2nd quarter", "3rd quarter", "4th quarter",
    "1st period", "2nd period", "3rd period",
];

/// Normalize a string for comparison
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Tokenize into words
fn tokenize(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .map(|w| w.to_string())
        .collect()
}

/// Check if text contains phrase as whole words (not substring of another word)
fn contains_phrase(text: &str, phrase: &str) -> bool {
    let text_words: Vec<&str> = text.split_whitespace().collect();
    let phrase_words: Vec<&str> = phrase.split_whitespace().collect();

    if phrase_words.is_empty() {
        return false;
    }

    // Look for contiguous phrase match
    if phrase_words.len() > 1 {
        for window in text_words.windows(phrase_words.len()) {
            if window == phrase_words.as_slice() {
                return true;
            }
        }
        return false;
    }

    // For single word phrases, check if it's a complete word in the text
    text_words.contains(&phrase_words[0])
}

/// Check if target matches candidate using alias lookup
fn check_alias_match(
    target_norm: &str,
    candidate_norm: &str,
    aliases: &HashMap<&str, Vec<&str>>,
) -> Option<(f64, String)> {
    // Direct lookup: is target a canonical name?
    if let Some(alias_list) = aliases.get(target_norm) {
        // Check if candidate matches any alias
        for alias in alias_list {
            let alias_norm = normalize(alias);
            if candidate_norm == alias_norm {
                return Some((0.95, format!("Alias exact: {} -> {}", target_norm, alias)));
            }
            if contains_phrase(candidate_norm, &alias_norm) {
                return Some((0.9, format!("Alias in text: {} -> {}", target_norm, alias)));
            }
        }
        // Also check if candidate contains the canonical name
        if contains_phrase(candidate_norm, target_norm) {
            return Some((0.9, format!("Canonical in text: {}", target_norm)));
        }
    }

    // Reverse lookup: is target an alias for some canonical name?
    for (canonical, alias_list) in aliases.iter() {
        let canonical_norm = normalize(canonical);

        // Check if target matches this canonical or any of its aliases
        let target_matches_group =
            target_norm == canonical_norm || alias_list.iter().any(|a| normalize(a) == target_norm);

        if target_matches_group {
            // Now check if candidate matches canonical
            if candidate_norm == canonical_norm {
                return Some((
                    0.95,
                    format!("Reverse: {} -> canonical {}", target_norm, canonical),
                ));
            }
            if contains_phrase(candidate_norm, &canonical_norm) {
                return Some((
                    0.9,
                    format!(
                        "Reverse canonical in text: {} -> {}",
                        target_norm, canonical
                    ),
                ));
            }

            // Check if candidate matches any alias in this group
            for alias in alias_list {
                let alias_norm = normalize(alias);
                if candidate_norm == alias_norm {
                    return Some((0.95, format!("Reverse alias: {} -> {}", target_norm, alias)));
                }
                if contains_phrase(candidate_norm, &alias_norm) {
                    return Some((
                        0.9,
                        format!("Reverse alias in text: {} -> {}", target_norm, alias),
                    ));
                }
            }
        }
    }

    None
}

/// Check if a word is too generic to be a reliable identifier
fn is_generic_word(word: &str) -> bool {
    GENERIC_WORDS.contains(&word)
}

/// Match a team name against text (like a market question).
/// Returns a MatchResult with confidence level and score.
pub fn match_team_in_text(team_name: &str, text: &str, sport: &str) -> MatchResult {
    let t_norm = normalize(team_name);
    let text_norm = normalize(text);

    if t_norm.is_empty() || text_norm.is_empty() {
        return MatchResult::none();
    }

    // Pre-check: tokenize and filter generic words early
    let t_words = tokenize(team_name);
    let text_words = tokenize(text);

    // Filter out generic words from target for matching purposes
    let t_significant: Vec<&String> = t_words.iter().filter(|w| !is_generic_word(w)).collect();

    // If the team name consists only of generic words, don't match
    if t_significant.is_empty() {
        return MatchResult::none();
    }

    // 1. Exact phrase match (highest confidence)
    // Only check if team name has at least one significant word
    if contains_phrase(&text_norm, &t_norm) {
        return MatchResult::exact();
    }

    // 2. Alias-based matching (sport-specific)
    let aliases = get_team_aliases(sport);
    if let Some((score, reason)) = check_alias_match(&t_norm, &text_norm, &aliases) {
        return MatchResult::high(score, &reason);
    }

    // For team names with mascot (e.g., "Cleveland Cavaliers"):
    // - Prefer matching the LAST significant word (usually the mascot)
    // - Mascots are more unique than city names
    if t_significant.len() >= 1 {
        // Get the last significant word (likely the mascot/nickname)
        if let Some(mascot) = t_significant.last() {
            // Check it's not a common mascot shared across many teams
            let shared_mascots = [
                "tigers",
                "wildcats",
                "bulldogs",
                "eagles",
                "cardinals",
                "panthers",
            ];

            if !shared_mascots.contains(&mascot.as_str()) {
                if text_words.contains(mascot) {
                    return MatchResult::high(0.85, &format!("Mascot match: {}", mascot));
                }
            } else if t_significant.len() >= 2 {
                // For shared mascots, require at least one more significant word to match
                let other_matches: usize = t_significant
                    .iter()
                    .filter(|w| *w != mascot && text_words.contains(*w))
                    .count();
                if other_matches >= 1 && text_words.contains(mascot) {
                    return MatchResult::high(
                        0.85,
                        &format!(
                            "Team + mascot: {} + {}",
                            t_significant
                                .iter()
                                .find(|w| *w != mascot && text_words.contains(*w))
                                .map(|s| s.as_str())
                                .unwrap_or(""),
                            mascot
                        ),
                    );
                }
            }
        }
    }

    // For single-word team names (e.g., "Lakers", "Celtics")
    if t_significant.len() == 1 {
        let word = &t_significant[0];
        if text_words.contains(word) {
            return MatchResult::high(0.85, &format!("Single word: {}", word));
        }
    }

    // Multi-word overlap: require strong overlap
    // Must match the majority of significant words, with at least 2 matches
    let match_count: usize = t_significant
        .iter()
        .filter(|w| text_words.contains(*w))
        .count();

    let overlap_ratio = match_count as f64 / t_significant.len() as f64;

    if match_count >= 2 && overlap_ratio >= 0.5 {
        return MatchResult::medium(
            overlap_ratio,
            &format!("Word overlap: {}/{}", match_count, t_significant.len()),
        );
    }

    // 4. Fuzzy match - ONLY for single words that are reasonably long
    // Using VERY high threshold to minimize false positives
    if t_significant.len() == 1 && t_norm.len() >= 6 {
        // Try to fuzzy match the single significant word against text words
        let word = &t_significant[0];
        for tw in &text_words {
            if tw.len() >= 5 {
                let score = jaro_winkler(word, tw);
                if score > 0.95 {
                    // Very high threshold
                    return MatchResult::medium(score, &format!("Fuzzy: {} ~ {}", word, tw));
                }
            }
        }
    }

    MatchResult::none()
}

/// Legacy API: Check if two team names match (for backwards compatibility).
pub fn names_match(target: &str, candidate: &str, sport: &str) -> bool {
    match_team_in_text(target, candidate, sport).is_match()
}

/// Match a game (home + away team) against market text.
/// Returns (is_match, home_result, away_result).
/// Only returns true if BOTH teams are found with sufficient confidence.
pub fn match_game_in_text(
    home_team: &str,
    away_team: &str,
    home_abbr: &str,
    away_abbr: &str,
    text: &str,
    sport: &str,
) -> (bool, MatchResult, MatchResult) {
    // Try to match home team (full name or abbreviation)
    let home_result = match_team_in_text(home_team, text, sport);
    let home_abbr_result = match_team_in_text(home_abbr, text, sport);
    let best_home = if home_result.score >= home_abbr_result.score {
        home_result
    } else {
        home_abbr_result
    };

    // Try to match away team (full name or abbreviation)
    let away_result = match_team_in_text(away_team, text, sport);
    let away_abbr_result = match_team_in_text(away_abbr, text, sport);
    let best_away = if away_result.score >= away_abbr_result.score {
        away_result
    } else {
        away_abbr_result
    };

    // Both teams must match for a valid game match
    let is_match = best_home.is_match() && best_away.is_match();

    (is_match, best_home, best_away)
}

/// Check if a market question indicates a non-moneyline market.
pub fn is_non_moneyline_market(question: &str) -> bool {
    let q = question.to_lowercase();

    // Check for non-moneyline indicators
    for indicator in NON_MONEYLINE_INDICATORS {
        if q.contains(indicator) {
            return true;
        }
    }

    // Check for spread patterns like "+5.5" or "-3.5"
    // Look for +/- followed by a digit (but not in a time like "7:30")
    let chars: Vec<char> = q.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if (c == '+' || c == '-') && i > 0 {
            // Check if preceded by a space (not part of a word or time)
            if chars[i - 1] == ' ' {
                // Check if followed by a digit
                if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    return true;
                }
            }
        }
    }

    false
}

// =============================================================================
// CONTEXT VALIDATION HELPER FUNCTIONS
// =============================================================================

/// Get sport-specific scoring characteristics for validation tolerances.
/// These help determine how meaningful score differences are for each sport.
pub fn get_sport_scoring(sport: &str) -> SportScoring {
    match sport.to_lowercase().as_str() {
        "nba" => SportScoring {
            typical_total: 220.0, // Average combined score ~220
            meaningful_margin: 10, // 10 point lead is significant
            score_volatility: 0.7, // High scoring, lots of variance
        },
        "nfl" => SportScoring {
            typical_total: 45.0, // Average combined score ~45
            meaningful_margin: 7, // One touchdown
            score_volatility: 0.5,
        },
        "nhl" => SportScoring {
            typical_total: 6.0, // Average combined score ~6
            meaningful_margin: 2, // 2 goals is meaningful
            score_volatility: 0.3, // Lower scoring, less variance
        },
        "mlb" => SportScoring {
            typical_total: 9.0, // Average combined runs ~9
            meaningful_margin: 3, // 3 runs is meaningful
            score_volatility: 0.4,
        },
        "ncaab" => SportScoring {
            typical_total: 140.0,
            meaningful_margin: 8,
            score_volatility: 0.6,
        },
        "ncaaf" => SportScoring {
            typical_total: 55.0,
            meaningful_margin: 7,
            score_volatility: 0.5,
        },
        "soccer" | "mls" => SportScoring {
            typical_total: 2.5, // Average combined goals ~2.5
            meaningful_margin: 1, // 1 goal is huge
            score_volatility: 0.2, // Low scoring
        },
        _ => SportScoring::default(),
    }
}

/// Parse game progress as a fraction (0.0 = start, 1.0 = end).
/// Returns None if progress cannot be determined.
pub fn parse_game_progress(
    period: Option<&str>,
    time_remaining: Option<&str>,
    sport: &str,
) -> Option<f64> {
    let period = period?;
    let sport_lower = sport.to_lowercase();

    // Get total periods and period length for this sport
    let (total_periods, period_minutes) = match sport_lower.as_str() {
        "nba" => (4, 12.0),
        "nfl" => (4, 15.0),
        "nhl" => (3, 20.0),
        "mlb" => (9, 0.0), // Innings, no time
        "ncaab" => (2, 20.0), // Two halves
        "ncaaf" => (4, 15.0),
        "soccer" | "mls" => (2, 45.0),
        _ => return None,
    };

    // Parse period number
    let period_num = parse_period_number(period, &sport_lower)?;

    // For baseball, we don't use time
    if sport_lower == "mlb" {
        return Some(period_num as f64 / total_periods as f64);
    }

    // Parse time remaining
    let time_fraction = if let Some(time_str) = time_remaining {
        parse_time_remaining(time_str, period_minutes)
    } else {
        0.5 // Assume midway through period if unknown
    };

    // Calculate total progress
    // Each period is (1/total_periods) of the game
    let completed_periods = (period_num - 1) as f64;
    let current_period_progress = 1.0 - time_fraction; // time_remaining decreases as period progresses

    let progress = (completed_periods + current_period_progress) / total_periods as f64;
    Some(progress.clamp(0.0, 1.0))
}

/// Parse period identifier to a number (1-indexed).
fn parse_period_number(period: &str, sport: &str) -> Option<u32> {
    let p = period.to_lowercase();

    // Try common formats: "Q1", "1st", "1", "2Q", "2nd Quarter", etc.
    // Also handle halves for sports that use them
    if p.contains("ot") || p.contains("overtime") {
        // Overtime - treat as beyond regulation
        return Some(5); // Will be clamped to end of game
    }

    // Half-based sports (soccer, ncaab)
    if sport == "soccer" || sport == "mls" || sport == "ncaab" {
        if p.contains("1h") || p.contains("1st") || p.contains("first") {
            return Some(1);
        }
        if p.contains("2h") || p.contains("2nd") || p.contains("second") {
            return Some(2);
        }
        if p.contains("half") {
            if p.contains('1') || p.contains("first") {
                return Some(1);
            }
            if p.contains('2') || p.contains("second") {
                return Some(2);
            }
        }
    }

    // Quarter-based sports
    if p.starts_with('q') {
        return p.chars().nth(1).and_then(|c| c.to_digit(10));
    }

    // Numeric with suffix: "1st", "2nd", "3rd", "4th"
    if let Some(num) = p.chars().next().and_then(|c| c.to_digit(10)) {
        return Some(num);
    }

    // Period for hockey: "P1", "1P", "1st Period"
    if p.contains("period") || p.starts_with('p') {
        for c in p.chars() {
            if let Some(num) = c.to_digit(10) {
                return Some(num);
            }
        }
    }

    // Inning for baseball
    if p.contains("inning") || p.contains("top") || p.contains("bot") {
        for c in p.chars() {
            if let Some(num) = c.to_digit(10) {
                return Some(num);
            }
        }
    }

    None
}

/// Parse time remaining string to fraction of period (1.0 = full period, 0.0 = end).
fn parse_time_remaining(time_str: &str, period_minutes: f64) -> f64 {
    let time_clean = time_str.trim();

    // Common formats: "12:00", "5:32", "0:45", "45:00+2" (soccer stoppage)
    let parts: Vec<&str> = time_clean
        .split(|c| c == ':' || c == '+')
        .collect();

    if parts.is_empty() {
        return 0.5;
    }

    let minutes: f64 = parts[0].parse().unwrap_or(0.0);
    let seconds: f64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);

    let total_seconds = minutes * 60.0 + seconds;
    let period_seconds = period_minutes * 60.0;

    if period_seconds <= 0.0 {
        return 0.5;
    }

    (total_seconds / period_seconds).clamp(0.0, 1.0)
}

/// Calculate time-based tolerance multiplier for score validation.
/// Earlier in game = more tolerance (scores can diverge from market description).
/// Later in game = less tolerance (scores should be close to what market shows).
pub fn calculate_time_tolerance(
    period: Option<&str>,
    time_remaining: Option<&str>,
    sport: &str,
) -> f64 {
    let progress = parse_game_progress(period, time_remaining, sport).unwrap_or(0.5);

    // Early game (0-25%): high tolerance (2.0x)
    // Mid game (25-75%): moderate tolerance (1.5x -> 1.0x)
    // Late game (75-100%): low tolerance (1.0x -> 0.5x)
    if progress < 0.25 {
        2.0
    } else if progress < 0.75 {
        // Linear interpolation from 1.5 to 1.0
        1.5 - (progress - 0.25) * 1.0
    } else {
        // Linear interpolation from 1.0 to 0.5
        1.0 - (progress - 0.75) * 2.0
    }
}

/// Extract scores from text (e.g., "Lakers 105 - Celtics 98", "105-98").
/// Returns (first_score, second_score) if found.
pub fn extract_scores_from_text(text: &str) -> Option<(u32, u32)> {
    // Pattern 1: "Team1 XXX - Team2 YYY" or "Team1 XXX vs Team2 YYY"
    // Pattern 2: "XXX-YYY" or "XXX - YYY"

    let text_clean = text.to_lowercase();

    // Look for score-like patterns: digits followed by separator followed by digits
    // Common separators: "-", " - ", "to", "vs"

    // Regex-like approach without regex crate:
    // Find sequences of digits separated by common delimiters
    let chars: Vec<char> = text_clean.chars().collect();
    let mut scores: Vec<u32> = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        // Look for digit sequences
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let num_str: String = chars[start..i].iter().collect();
            if let Ok(num) = num_str.parse::<u32>() {
                // Filter out unlikely scores (years, times, etc.)
                // Valid scores are typically 0-200 for most sports
                if num <= 200 {
                    scores.push(num);
                }
            }
        } else {
            i += 1;
        }
    }

    // Return first two valid scores found
    if scores.len() >= 2 {
        Some((scores[0], scores[1]))
    } else {
        None
    }
}

/// Validate if game scores correlate with scores mentioned in market text.
/// Returns confidence score (0.0-1.0) or None if scores not extractable.
pub fn validate_score_correlation(
    game_context: Option<&GameContext>,
    market_context: Option<&MarketContext>,
    sport: &str,
) -> Option<f64> {
    let game_ctx = game_context?;
    let market_ctx = market_context?;

    // Need actual scores from game
    let game_home = game_ctx.home_score?;
    let game_away = game_ctx.away_score?;

    // Try to extract scores from market title
    let market_title = market_ctx.market_title.as_ref()?;
    let (market_score1, market_score2) = extract_scores_from_text(market_title)?;

    // Get sport-specific tolerances
    let scoring = get_sport_scoring(sport);
    let time_tolerance = calculate_time_tolerance(
        game_ctx.period.as_deref(),
        game_ctx.time_remaining.as_deref(),
        sport,
    );

    // Calculate acceptable deviation
    // Base tolerance is meaningful_margin, scaled by time_tolerance
    let acceptable_diff = (scoring.meaningful_margin as f64 * time_tolerance) as u32;

    // Check if scores match (in either order since we don't know which is which)
    let match1 = score_diff(game_home, market_score1) <= acceptable_diff
        && score_diff(game_away, market_score2) <= acceptable_diff;
    let match2 = score_diff(game_home, market_score2) <= acceptable_diff
        && score_diff(game_away, market_score1) <= acceptable_diff;

    if match1 || match2 {
        // Calculate how close the match is
        let diff1 = score_diff(game_home, market_score1) + score_diff(game_away, market_score2);
        let diff2 = score_diff(game_home, market_score2) + score_diff(game_away, market_score1);
        let min_diff = diff1.min(diff2);

        // Convert to confidence: 0 diff = 1.0, acceptable_diff*2 = 0.5
        let max_total_diff = acceptable_diff * 2;
        let confidence = if max_total_diff == 0 {
            1.0
        } else {
            1.0 - (min_diff as f64 / (max_total_diff * 2) as f64)
        };

        Some(confidence.clamp(0.0, 1.0))
    } else {
        Some(0.0) // Scores don't match
    }
}

/// Helper to calculate absolute difference between scores.
fn score_diff(a: u32, b: u32) -> u32 {
    if a > b { a - b } else { b - a }
}

/// Validate that the opponent team appears in the market context.
/// Returns confidence score (0.0-1.0).
pub fn validate_opponent(
    game_context: Option<&GameContext>,
    market_context: Option<&MarketContext>,
    target_is_home: bool,
    sport: &str,
) -> f64 {
    let game_ctx = match game_context {
        Some(ctx) => ctx,
        None => return 1.0, // No context = assume valid
    };

    let market_ctx = match market_context {
        Some(ctx) => ctx,
        None => return 1.0, // No context = assume valid
    };

    // Get opponent name from game context
    let opponent = if target_is_home {
        &game_ctx.away_team
    } else {
        &game_ctx.home_team
    };

    if opponent.is_empty() {
        return 1.0; // Can't validate without opponent
    }

    // Check if opponent appears in market participants
    for participant in &market_ctx.market_participants {
        let result = match_team_in_text(opponent, participant, sport);
        if result.is_match() {
            return result.score;
        }
    }

    // Check if opponent appears in market title
    if let Some(title) = &market_ctx.market_title {
        let result = match_team_in_text(opponent, title, sport);
        if result.is_match() {
            return result.score;
        }
    }

    0.0 // Opponent not found - suspicious
}

// =============================================================================
// CONTEXT-ENHANCED MATCHING (PRIMARY API)
// =============================================================================

/// Match teams with full game/market context validation.
///
/// This is the enhanced matching function that combines name-based matching
/// with context validation (sport, opponent, score correlation).
///
/// # Arguments
/// * `target_team` - The team name we're looking for
/// * `candidate_team` - The text to search in (e.g., market question)
/// * `sport` - Sport identifier (e.g., "nba", "nhl")
/// * `game_context` - Optional game state for enhanced validation
/// * `market_context` - Optional market metadata for enhanced validation
/// * `target_is_home` - Whether target_team is the home team (for opponent validation)
///
/// # Returns
/// `ContextMatchResult` with confidence scores for each validation layer.
/// When no context is provided, behaves like `match_team_in_text()`.
pub fn match_teams_with_context(
    target_team: &str,
    candidate_team: &str,
    sport: &str,
    game_context: Option<&GameContext>,
    market_context: Option<&MarketContext>,
    target_is_home: bool,
) -> ContextMatchResult {
    // 1. Do basic name matching
    let name_match = match_team_in_text(target_team, candidate_team, sport);
    let name_summary = NameMatchSummary::from(&name_match);

    // If name doesn't match, we're done (no point validating context)
    if !name_match.is_match() {
        return ContextMatchResult {
            name_match: name_summary,
            sport_valid: true, // N/A
            opponent_score: 0.0,
            score_correlation: None,
            final_confidence: 0.0,
            rejection_reason: Some("Name match failed".to_string()),
        };
    }

    // 2. Sport validation (if market_context provides sport info)
    let sport_valid = validate_sport_match(game_context, market_context);

    // Early rejection on sport mismatch
    if !sport_valid {
        return ContextMatchResult {
            name_match: name_summary,
            sport_valid: false,
            opponent_score: 0.0,
            score_correlation: None,
            final_confidence: 0.0,
            rejection_reason: Some("Sport mismatch between game and market".to_string()),
        };
    }

    // 3. Opponent validation (if both contexts provided)
    let opponent_score = validate_opponent(game_context, market_context, target_is_home, sport);

    // Low opponent score is a warning but not immediate rejection
    // (the opponent might just not be mentioned in a single-team market question)

    // 4. Score correlation (if available)
    let score_correlation = validate_score_correlation(game_context, market_context, sport);

    // 5. Calculate final confidence - weighted combination of all factors
    let final_confidence = calculate_combined_confidence(
        name_match.score,
        sport_valid,
        opponent_score,
        score_correlation,
    );

    // Determine if there's a rejection reason
    let rejection_reason = if final_confidence < 0.5 {
        if opponent_score < 0.3 {
            Some("Opponent not found in market".to_string())
        } else if score_correlation.map(|s| s < 0.3).unwrap_or(false) {
            Some("Score mismatch".to_string())
        } else {
            Some("Combined confidence too low".to_string())
        }
    } else {
        None
    };

    ContextMatchResult {
        name_match: name_summary,
        sport_valid,
        opponent_score,
        score_correlation,
        final_confidence,
        rejection_reason,
    }
}

/// Validate that game sport matches market sport.
fn validate_sport_match(
    game_context: Option<&GameContext>,
    market_context: Option<&MarketContext>,
) -> bool {
    // If either context is missing, assume valid (backward compatible)
    let game_ctx = match game_context {
        Some(ctx) => ctx,
        None => return true,
    };

    let market_ctx = match market_context {
        Some(ctx) => ctx,
        None => return true,
    };

    // If market doesn't specify sport, assume valid
    let market_sport = match &market_ctx.market_sport {
        Some(s) if !s.is_empty() => s.to_lowercase(),
        _ => return true,
    };

    let game_sport = game_ctx.sport.to_lowercase();

    // Handle sport aliases
    let game_normalized = normalize_sport_name(&game_sport);
    let market_normalized = normalize_sport_name(&market_sport);

    game_normalized == market_normalized
}

/// Normalize sport name to handle common variations.
fn normalize_sport_name(sport: &str) -> &str {
    match sport {
        "nba" | "basketball" => "nba",
        "nfl" | "football" | "pro-football" => "nfl",
        "nhl" | "hockey" | "ice-hockey" => "nhl",
        "mlb" | "baseball" => "mlb",
        "ncaab" | "college-basketball" | "mens-college-basketball" => "ncaab",
        "ncaaf" | "college-football" => "ncaaf",
        "soccer" | "mls" | "football-soccer" | "epl" | "premier-league" => "soccer",
        "mma" | "ufc" => "mma",
        "tennis" | "atp" | "wta" => "tennis",
        other => other,
    }
}

/// Calculate combined confidence from all validation factors.
fn calculate_combined_confidence(
    name_score: f64,
    sport_valid: bool,
    opponent_score: f64,
    score_correlation: Option<f64>,
) -> f64 {
    // If sport doesn't match, confidence is 0
    if !sport_valid {
        return 0.0;
    }

    // Weights for each factor
    const NAME_WEIGHT: f64 = 0.5;
    const OPPONENT_WEIGHT: f64 = 0.3;
    const SCORE_WEIGHT: f64 = 0.2;

    // If we don't have score correlation, redistribute its weight
    let (name_w, opponent_w, score_w) = if score_correlation.is_some() {
        (NAME_WEIGHT, OPPONENT_WEIGHT, SCORE_WEIGHT)
    } else {
        // No score data - redistribute to name and opponent
        (NAME_WEIGHT + SCORE_WEIGHT / 2.0, OPPONENT_WEIGHT + SCORE_WEIGHT / 2.0, 0.0)
    };

    let score_contrib = score_correlation.unwrap_or(0.0) * score_w;

    let combined = name_score * name_w + opponent_score * opponent_w + score_contrib;

    combined.clamp(0.0, 1.0)
}

// =============================================================================
// UNIT TESTS
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Sport Scoring Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sport_scoring_nhl() {
        let scoring = get_sport_scoring("nhl");
        assert_eq!(scoring.meaningful_margin, 2);
        assert!((scoring.typical_total - 6.0).abs() < 0.1);
    }

    #[test]
    fn test_sport_scoring_nba() {
        let scoring = get_sport_scoring("nba");
        assert_eq!(scoring.meaningful_margin, 10);
        assert!((scoring.typical_total - 220.0).abs() < 0.1);
    }

    #[test]
    fn test_sport_scoring_soccer() {
        let scoring = get_sport_scoring("soccer");
        assert_eq!(scoring.meaningful_margin, 1);
        assert!((scoring.typical_total - 2.5).abs() < 0.1);
    }

    #[test]
    fn test_sport_scoring_unknown_uses_default() {
        let scoring = get_sport_scoring("curling");
        assert_eq!(scoring.meaningful_margin, 5);
    }

    // -------------------------------------------------------------------------
    // Sport Mismatch Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sport_mismatch_rejection() {
        let game_ctx = GameContext {
            home_team: "Florida Panthers".to_string(),
            away_team: "Boston Bruins".to_string(),
            sport: "nhl".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_sport: Some("nfl".to_string()), // Wrong sport!
            market_title: Some("Panthers vs Bruins".to_string()),
            ..Default::default()
        };

        let result = match_teams_with_context(
            "Panthers",
            "Panthers vs Bruins",
            "nhl",
            Some(&game_ctx),
            Some(&market_ctx),
            true,
        );

        assert!(!result.sport_valid);
        assert_eq!(result.final_confidence, 0.0);
        assert!(result.rejection_reason.is_some());
    }

    #[test]
    fn test_sport_match_success() {
        let game_ctx = GameContext {
            home_team: "Boston Celtics".to_string(),
            away_team: "Los Angeles Lakers".to_string(),
            sport: "nba".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_sport: Some("nba".to_string()),
            market_title: Some("Celtics vs Lakers".to_string()),
            market_participants: vec!["Celtics".to_string(), "Lakers".to_string()],
        };

        let result = match_teams_with_context(
            "Boston Celtics",
            "Celtics vs Lakers",
            "nba",
            Some(&game_ctx),
            Some(&market_ctx),
            true,
        );

        assert!(result.sport_valid);
        assert!(result.name_match.is_match);
        assert!(result.final_confidence > 0.5);
    }

    // -------------------------------------------------------------------------
    // Backward Compatibility Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_context_optional_backward_compat() {
        // Without context, should work like match_team_in_text
        let result = match_teams_with_context(
            "Boston Celtics",
            "Who will win? Celtics vs Lakers",
            "nba",
            None,
            None,
            true,
        );

        assert!(result.name_match.is_match);
        assert!(result.sport_valid); // Defaults to true
        assert!(result.final_confidence > 0.0);
    }

    #[test]
    fn test_no_match_still_no_match_with_context() {
        let game_ctx = GameContext {
            home_team: "Miami Heat".to_string(),
            away_team: "Boston Celtics".to_string(),
            sport: "nba".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_sport: Some("nba".to_string()),
            market_title: Some("Lakers vs Warriors".to_string()),
            ..Default::default()
        };

        let result = match_teams_with_context(
            "Miami Heat",
            "Lakers vs Warriors", // Heat is not in this market!
            "nba",
            Some(&game_ctx),
            Some(&market_ctx),
            true,
        );

        assert!(!result.name_match.is_match);
        assert_eq!(result.final_confidence, 0.0);
    }

    // -------------------------------------------------------------------------
    // Game Progress Parsing Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_game_progress_nba_q1() {
        let progress = parse_game_progress(Some("Q1"), Some("6:00"), "nba");
        assert!(progress.is_some());
        // Q1 with 6:00 remaining = about 12.5% through the game
        let p = progress.unwrap();
        assert!(p >= 0.0 && p <= 0.25);
    }

    #[test]
    fn test_parse_game_progress_nba_q4() {
        let progress = parse_game_progress(Some("Q4"), Some("2:00"), "nba");
        assert!(progress.is_some());
        // Q4 with 2:00 remaining = about 95% through the game
        let p = progress.unwrap();
        assert!(p >= 0.75 && p <= 1.0);
    }

    #[test]
    fn test_parse_game_progress_soccer_1h() {
        let progress = parse_game_progress(Some("1H"), Some("20:00"), "soccer");
        assert!(progress.is_some());
        // 1st half with 20:00 played (45 min half) = about 22%
        let p = progress.unwrap();
        assert!(p >= 0.0 && p <= 0.5);
    }

    #[test]
    fn test_parse_game_progress_unknown_sport() {
        let progress = parse_game_progress(Some("Q1"), Some("5:00"), "curling");
        assert!(progress.is_none());
    }

    // -------------------------------------------------------------------------
    // Score Extraction Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_extract_scores_dash_format() {
        let scores = extract_scores_from_text("Lakers 105 - Celtics 98");
        assert!(scores.is_some());
        let (s1, s2) = scores.unwrap();
        assert_eq!(s1, 105);
        assert_eq!(s2, 98);
    }

    #[test]
    fn test_extract_scores_simple_format() {
        let scores = extract_scores_from_text("105-98");
        assert!(scores.is_some());
        let (s1, s2) = scores.unwrap();
        assert_eq!(s1, 105);
        assert_eq!(s2, 98);
    }

    #[test]
    fn test_extract_scores_no_scores() {
        let scores = extract_scores_from_text("Lakers vs Celtics: Who will win?");
        assert!(scores.is_none());
    }

    #[test]
    fn test_extract_scores_filters_year() {
        // 2024 should be filtered out as it's > 200
        let scores = extract_scores_from_text("NBA Finals 2024: 105-98");
        assert!(scores.is_some());
        let (s1, s2) = scores.unwrap();
        assert_eq!(s1, 105);
        assert_eq!(s2, 98);
    }

    // -------------------------------------------------------------------------
    // Opponent Validation Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_opponent_found_in_participants() {
        let game_ctx = GameContext {
            home_team: "Boston Celtics".to_string(),
            away_team: "Los Angeles Lakers".to_string(),
            sport: "nba".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_participants: vec!["Celtics".to_string(), "Lakers".to_string()],
            ..Default::default()
        };

        // Looking for home team, opponent is away team (Lakers)
        let score = validate_opponent(Some(&game_ctx), Some(&market_ctx), true, "nba");
        assert!(score > 0.5);
    }

    #[test]
    fn test_opponent_not_found() {
        let game_ctx = GameContext {
            home_team: "Boston Celtics".to_string(),
            away_team: "Los Angeles Lakers".to_string(),
            sport: "nba".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_participants: vec!["Warriors".to_string(), "Suns".to_string()],
            ..Default::default()
        };

        // Looking for home team, but opponent (Lakers) not in market
        let score = validate_opponent(Some(&game_ctx), Some(&market_ctx), true, "nba");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_opponent_no_context_returns_valid() {
        let score = validate_opponent(None, None, true, "nba");
        assert_eq!(score, 1.0);
    }

    // -------------------------------------------------------------------------
    // Combined Confidence Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_combined_confidence_all_high() {
        let conf = calculate_combined_confidence(0.95, true, 0.9, Some(0.85));
        assert!(conf > 0.8);
    }

    #[test]
    fn test_combined_confidence_sport_mismatch() {
        let conf = calculate_combined_confidence(0.95, false, 0.9, Some(0.85));
        assert_eq!(conf, 0.0);
    }

    #[test]
    fn test_combined_confidence_no_score_data() {
        // Without score correlation, weights are redistributed
        let conf = calculate_combined_confidence(0.9, true, 0.8, None);
        assert!(conf > 0.7);
    }

    // -------------------------------------------------------------------------
    // Integration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_full_context_match_nba() {
        let game_ctx = GameContext {
            home_team: "Boston Celtics".to_string(),
            away_team: "Los Angeles Lakers".to_string(),
            home_score: Some(85),
            away_score: Some(82),
            period: Some("Q3".to_string()),
            time_remaining: Some("4:30".to_string()),
            sport: "nba".to_string(),
        };
        let market_ctx = MarketContext {
            market_title: Some("Will the Boston Celtics beat the Lakers?".to_string()),
            market_sport: Some("nba".to_string()),
            market_participants: vec!["Celtics".to_string(), "Lakers".to_string()],
        };

        let result = match_teams_with_context(
            "Boston Celtics",
            "Will the Boston Celtics beat the Lakers?",
            "nba",
            Some(&game_ctx),
            Some(&market_ctx),
            true,
        );

        assert!(result.name_match.is_match);
        assert!(result.sport_valid);
        assert!(result.opponent_score > 0.5);
        assert!(result.final_confidence > 0.5);
        assert!(result.rejection_reason.is_none());
    }

    #[test]
    fn test_cross_league_rejection_panthers() {
        // Florida Panthers (NHL) should NOT match Carolina Panthers (NFL)
        let game_ctx = GameContext {
            home_team: "Florida Panthers".to_string(),
            away_team: "Boston Bruins".to_string(),
            sport: "nhl".to_string(),
            ..Default::default()
        };
        let market_ctx = MarketContext {
            market_title: Some("Carolina Panthers vs Atlanta Falcons".to_string()),
            market_sport: Some("nfl".to_string()), // NFL, not NHL!
            market_participants: vec!["Panthers".to_string(), "Falcons".to_string()],
        };

        let result = match_teams_with_context(
            "Panthers",
            "Carolina Panthers vs Atlanta Falcons",
            "nhl",
            Some(&game_ctx),
            Some(&market_ctx),
            true,
        );

        // Should be rejected due to sport mismatch
        assert!(!result.sport_valid);
        assert_eq!(result.final_confidence, 0.0);
    }
}
