use strsim::jaro_winkler;

/// Normalized comparison of two team names
pub fn names_match(target: &str, candidate: &str, sport: &str) -> bool {
    let t_norm = normalize(target);
    let c_norm = normalize(candidate);

    // 1. Exact match (fast path)
    if t_norm == c_norm {
        return true;
    }

    // 2. Sport-Specific Aliases
    let aliases: &[(&str, &str)] = match sport {
        "nba" => &[
            ("sixers", "76ers"),
            ("phi", "76ers"),
            ("phi", "sixers"),
            ("cavs", "cavaliers"),
            ("cle", "cavaliers"),
            ("mavs", "mavericks"),
            ("dal", "mavericks"),
            ("warriors", "gsw"),
            ("warriors", "golden state"),
            ("gs", "warriors"),
            ("knicks", "new york knicks"),
            ("nyk", "knicks"),
            ("wolves", "timberwolves"),
            ("min", "timberwolves"),
            ("blazers", "trail blazers"),
            ("por", "trail blazers"),
            ("bucks", "mil"),
            ("bulls", "chi"),
            ("celtics", "bos"),
            ("clippers", "lac"),
            ("grizzlies", "mem"),
            ("hawks", "atl"),
            ("heat", "mia"),
            ("hornets", "cha"),
            ("jazz", "uta"),
            ("kings", "sac"),
            ("lakers", "lal"),
            ("magic", "orl"),
            ("nets", "bkn"),
            ("nuggets", "den"),
            ("pacers", "ind"),
            ("pelicans", "nop"),
            ("pistons", "det"),
            ("raptors", "tor"),
            ("rockets", "hou"),
            ("spurs", "sas"),
            ("suns", "phx"),
            ("thunder", "okc"),
            ("wizards", "was"),
            ("76ers", "philadelphia"),
            ("sixers", "philadelphia"),
        ],
        "ncaab" | "ncaaf" => &[
            ("uconn", "connecticut"),
            ("bama", "alabama"),
            ("nd", "notre dame"),
            ("lsu", "louisiana state"),
            ("ole miss", "mississippi"),
            ("nc state", "north carolina state"),
            ("unc", "north carolina"),
            ("fsu", "florida state"),
            ("uva", "virginia"),
            ("vt", "virginia tech"),
            ("wvu", "west virginia"),
            ("penn st", "penn state"),
            ("msu", "michigan state"),
            ("osu", "ohio state"),
            ("ou", "oklahoma"),
            ("tex", "texas"),
            ("aggies", "texas am"),
            ("vols", "tennessee"),
            ("cuse", "syracuse"),
            ("canes", "miami"),
            ("gators", "florida"),
            ("huskies", "washington"),
            ("ducks", "oregon"),
            ("buckeyes", "ohio state"),
        ],
        "nhl" => &[
            ("jackets", "blue jackets"),
            ("wings", "red wings"),
            ("caps", "capitals"),
            ("pens", "penguins"),
            ("isles", "islanders"),
            ("leafs", "maple leafs"),
            ("knights", "golden knights"),
            ("hawks", "blackhawks"),
            ("preds", "predators"),
            ("avs", "avalanche"),
            ("bolts", "lightning"),
            ("canes", "hurricanes"),
            ("devils", "nj"),
            ("habs", "canadiens"),
            ("sens", "senators"),
            ("yotes", "coyotes"),
            ("sabres", "buf"),
        ],
        "soccer" => &[
            ("man city", "manchester city"),
            ("man utd", "manchester united"),
            ("spurs", "tottenham"),
            ("wolves", "wolverhampton"),
            ("palace", "crystal palace"),
            ("forest", "nottingham forest"),
            ("leeds", "leeds united"),
            ("psg", "paris saint germain"),
            ("bayern", "bayern munich"),
            ("real", "real madrid"),
            ("atletico", "atletico madrid"),
            ("dortmund", "borussia dortmund"),
            ("inter", "internazionale"),
            ("milan", "ac milan"),
            ("barca", "barcelona"),
            ("leicester", "leicester city"),
            ("villa", "aston villa"),
        ],
        _ => &[],
    };

    for (a1, a2) in aliases {
        // If target contains a1, candidate contains a2 (and vice versa)
        if (t_norm.contains(a1) && c_norm.contains(a2))
            || (t_norm.contains(a2) && c_norm.contains(a1))
        {
            return true;
        }
    }

    // 3. Smart Word-Boundary / Multi-word Match
    let t_words: Vec<&str> = t_norm.split_whitespace().collect();
    let c_words: Vec<&str> = c_norm.split_whitespace().collect();

    // Special case for numbers: if one name has "76ers" and other has "76ers"
    for tw in &t_words {
        if tw.chars().any(|c| c.is_numeric()) {
            for cw in &c_words {
                if tw == cw {
                    return true;
                }
            }
        }
    }

    // If one is a single word, it must be a full word in the other
    if t_words.len() == 1 {
        if c_words.contains(&t_words[0]) {
            return true;
        }
    }
    if c_words.len() == 1 {
        if t_words.contains(&c_words[0]) {
            return true;
        }
    }

    // Overlap Check (e.g. "Los Angeles Lakers" vs "Lakers")
    let shorter = if t_words.len() < c_words.len() {
        &t_words
    } else {
        &c_words
    };
    let longer = if t_words.len() < c_words.len() {
        &c_words
    } else {
        &t_words
    };

    let mut matches = 0;
    for &w in shorter {
        if longer.contains(&w) {
            matches += 1;
        }
    }
    if matches > 0 && matches >= (shorter.len() + 1) / 2 {
        return true;
    }

    // 4. Fuzzy match (Jaro-Winkler)
    let score = jaro_winkler(&t_norm, &c_norm);
    if score > 0.88 {
        return true;
    }

    false
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matching() {
        assert!(names_match(
            "Notre Dame",
            "Notre Dame Fighting Irish",
            "ncaaf"
        ));
        assert!(names_match("Lakers", "Los Angeles Lakers", "nba"));
        assert!(names_match("UConn", "uconn", "ncaab")); // Case insensitive
        assert!(names_match("Sixers", "76ers", "nba"));
        assert!(names_match("Man City", "Manchester City", "soccer"));
        assert!(names_match("Caps", "Washington Capitals", "nhl"));
        assert!(names_match("Bama", "Alabama", "ncaaf"));
    }
}
