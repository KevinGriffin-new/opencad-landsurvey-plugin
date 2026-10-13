//! Feature-code library in MicroSurvey's AutoMAP CSV format: one row per
//! code, saying whether its points connect into linework and how that
//! linework is drawn (layer, linetype, colour, 2D/3D, width), plus the symbol
//! and description settings later field-to-finish phases use. Columns are
//! found by header name, so libraries from any MicroSurvey version load.
//!
//! Lookup follows the AutoMAP help ("AutoMAP Library", "Extended Options"):
//! an exact match on the whole description wins, then its first word (text
//! after a space is ignored), then `*` wildcards (`PIN*`, `*PIN`, `*PIN*`) in
//! file order. As an extension for string-numbered coding, a first word whose
//! exact form is absent also matches with its string number removed, so a
//! `TC` row styles `TC1`, `TC2`, ….

/// One library row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Code {
    /// The description key, possibly with `*` wildcards (upper-cased).
    pub key: String,
    /// `CONNECT`: points with this code join into linework.
    pub connect: bool,
    /// `LINE LAYER` (empty: the plugin's default layer).
    pub line_layer: String,
    /// `LINE TYPE` (empty: by layer).
    pub line_type: String,
    /// `COLOR` as an AutoCAD colour index 1–255; `None` for by-layer (256),
    /// by-block (0) or blank.
    pub color: Option<i16>,
    /// `2D` = 0: draw the linework in 3D.
    pub three_d: bool,
    /// `PLINE WIDTH` for 2D polylines.
    pub width: f64,
    /// `EXCLUDE FROM DTM`.
    pub exclude_from_dtm: bool,
    /// `SYMBOL`, as written (e.g. `BLOCKS\ICM_FSIP.DWG`).
    pub symbol: String,
    /// `SYM SCALE` (0 is treated as 1, as AutoMAP does).
    pub symbol_scale: f64,
    /// `SYM LAYER`.
    pub symbol_layer: String,
    /// `SYM TRIM`: connecting-line trim radius.
    pub symbol_trim: f64,
    /// `DESC LAYER`.
    pub desc_layer: String,
    /// `DESC TEXT`: the plotted description.
    pub desc_text: String,
}

impl Code {
    fn wildcard(&self) -> bool {
        self.key.contains('*')
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Library {
    pub codes: Vec<Code>,
}

impl Library {
    /// Parse an AutoMAP library CSV. The header row must have a `KEY` column;
    /// every other column is optional.
    pub fn parse(text: &str) -> Result<Library, String> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header = lines.next().ok_or("the library file is empty")?;
        let header: Vec<String> = split_csv(header.trim_start_matches('\u{feff}'))
            .into_iter()
            .map(|h| h.trim().to_ascii_uppercase())
            .collect();
        let col = |name: &str| header.iter().position(|h| h == name);
        let key = col("KEY").ok_or("no KEY column: not an AutoMAP library")?;
        let cols = [
            "CONNECT", "LINE LAYER", "LINE TYPE", "COLOR", "2D", "PLINE WIDTH", "EXCLUDE FROM DTM",
            "SYMBOL", "SYM SCALE", "SYM LAYER", "SYM TRIM", "DESC LAYER", "DESC TEXT",
        ]
        .map(col);
        let mut codes = Vec::new();
        for line in lines {
            let f = split_csv(line);
            let get = |i: Option<usize>| i.and_then(|i| f.get(i)).map(|s| s.trim()).unwrap_or("");
            let num = |i: Option<usize>| get(i).parse::<f64>().unwrap_or(0.0);
            let flag = |i: Option<usize>| num(i) != 0.0;
            let k = get(Some(key)).to_ascii_uppercase();
            if k.is_empty() {
                continue;
            }
            let color = get(cols[3]).parse::<i16>().ok().filter(|c| (1..=255).contains(c));
            let scale = num(cols[8]);
            codes.push(Code {
                key: k,
                connect: flag(cols[0]),
                line_layer: get(cols[1]).to_string(),
                line_type: get(cols[2]).to_string(),
                color,
                // A blank 2D column draws in 2D, AutoMAP's default.
                three_d: get(cols[4]) == "0",
                width: num(cols[5]),
                exclude_from_dtm: flag(cols[6]),
                symbol: get(cols[7]).to_string(),
                symbol_scale: if scale == 0.0 { 1.0 } else { scale },
                symbol_layer: get(cols[9]).to_string(),
                symbol_trim: num(cols[10]),
                desc_layer: get(cols[11]).to_string(),
                desc_text: get(cols[12]).to_string(),
            });
        }
        if codes.is_empty() {
            return Err("the library has no codes".into());
        }
        Ok(Library { codes })
    }

    /// The row for a point description, as AutoMAP looks it up.
    pub fn lookup(&self, description: &str) -> Option<usize> {
        let full = description.trim().to_ascii_uppercase();
        let word = full.split_whitespace().next().unwrap_or("");
        self.exact(&full)
            .or_else(|| self.word(word))
            .or_else(|| self.wild(&full))
    }

    /// The row for one code word (no notes): exact, then without its string
    /// number, then by wildcard.
    pub fn lookup_word(&self, word: &str) -> Option<usize> {
        self.word(&word.to_ascii_uppercase())
    }

    /// Whether `word` is itself a (non-wildcard) code in the library.
    pub fn has_exact(&self, word: &str) -> bool {
        self.exact(&word.to_ascii_uppercase()).is_some()
    }

    fn word(&self, word: &str) -> Option<usize> {
        if word.is_empty() {
            return None;
        }
        let base = word.trim_end_matches(|c: char| c.is_ascii_digit());
        self.exact(word)
            .or_else(|| (base.len() < word.len() && !base.is_empty()).then(|| self.exact(base)).flatten())
            .or_else(|| self.wild(word))
    }

    fn exact(&self, s: &str) -> Option<usize> {
        self.codes.iter().position(|c| !c.wildcard() && c.key == s)
    }

    fn wild(&self, s: &str) -> Option<usize> {
        self.codes.iter().position(|c| c.wildcard() && glob(&c.key, s))
    }
}

/// `*`-only glob match.
fn glob(pattern: &str, s: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !s.starts_with(first) || s.len() < first.len() + last.len() || !s.ends_with(last) {
        return false;
    }
    let mut rest = &s[first.len()..s.len() - last.len()];
    for p in &parts[1..parts.len() - 1] {
        match rest.find(p) {
            Some(i) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    true
}

/// Split one CSV line, honouring double-quoted fields.
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "\u{feff}KEY,DESC TEXT,DESC LAYER,SYMBOL,SYM SCALE,SYM LAYER,SYM TRIM,CONNECT,LINE LAYER,LINE TYPE,COLOR,2D,LINES,PLINE WIDTH,EXCLUDE FROM DTM\n\
        TC,,,,0,,0,1,CURB TOP,,5,0,1,0,0\n\
        CL,,,,0,,0,1,ROAD CENTER,CENTER,256,0,1,0,0\n\
        SHED,,,,0,,0,1,BUILDING,,3,1,1,0.25,0\n\
        OIP,FOUND IRON POST,MON,BLOCKS\\ICM_FSIP.DWG,1,BLOCK,1,0,,,255,1,1,0,1\n\
        TREE 40FIR,,,,0,,0,0,,,0,1,1,0,0\n\
        PIN*,,,,0,,0,0,,,0,1,1,0,0\n\
        *WALL*,\"WALL, RETAINING\",,,0,,0,1,WALLS,,7,1,1,0,0\n";

    #[test]
    fn parses_automap_columns() {
        let lib = Library::parse(LIB).unwrap();
        assert_eq!(lib.codes.len(), 7);
        let tc = &lib.codes[0];
        assert_eq!((tc.key.as_str(), tc.connect, tc.line_layer.as_str(), tc.color, tc.three_d), ("TC", true, "CURB TOP", Some(5), true));
        let cl = &lib.codes[1];
        assert_eq!((cl.line_type.as_str(), cl.color), ("CENTER", None));
        let shed = &lib.codes[2];
        assert_eq!((shed.three_d, shed.width), (false, 0.25));
        let oip = &lib.codes[3];
        assert_eq!(oip.symbol, "BLOCKS\\ICM_FSIP.DWG");
        assert_eq!((oip.symbol_scale, oip.symbol_trim, oip.exclude_from_dtm), (1.0, 1.0, true));
        assert_eq!(lib.codes[0].symbol_scale, 1.0, "scale 0 means 1");
        assert_eq!(lib.codes[6].desc_text, "WALL, RETAINING");
    }

    #[test]
    fn lookup_follows_automap() {
        let lib = Library::parse(LIB).unwrap();
        let key = |d: &str| lib.lookup(d).map(|i| lib.codes[i].key.as_str());
        // Exact whole description, then first word.
        assert_eq!(key("TREE 40FIR"), Some("TREE 40FIR"));
        assert_eq!(key("TC east side"), Some("TC"));
        assert_eq!(key("tc"), Some("TC"));
        // String numbers drop to the base code.
        assert_eq!(key("TC12"), Some("TC"));
        // Wildcards, at the end and on both sides.
        assert_eq!(key("PIN"), Some("PIN*"));
        assert_eq!(key("PIN12345"), Some("PIN*"));
        assert_eq!(key("PIN Bent"), Some("PIN*"));
        assert_eq!(key("RETWALL2"), Some("*WALL*"));
        assert_eq!(key("G"), None);
        assert!(lib.has_exact("oip") && !lib.has_exact("PIN"));
    }

    #[test]
    fn rejects_non_libraries() {
        assert!(Library::parse("").is_err());
        assert!(Library::parse("NAME,X\nA,1\n").is_err());
        assert!(Library::parse("KEY,CONNECT\n").is_err());
    }

    #[test]
    fn glob_forms() {
        assert!(glob("PIN*", "PIN") && glob("*PIN", "XPIN") && glob("*PIN*", "APINB"));
        assert!(glob("A*B*C", "AxxBxxC") && !glob("A*B*C", "AxxC"));
        assert!(!glob("PIN*", "SPIN"));
    }
}
