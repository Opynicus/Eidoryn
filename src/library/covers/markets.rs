//! Catalog market names and the identifiers used by each cover service.

pub(super) struct Market {
    pub name: &'static str,
    pub identifier: &'static str,
    pub screenscraper: Option<&'static str>,
}

const fn market(
    name: &'static str,
    identifier: &'static str,
    screenscraper: Option<&'static str>,
) -> Market {
    Market {
        name,
        identifier,
        screenscraper,
    }
}

// ScreenScraper's regionsListe uses its own shortname, including "sp" for Spain.
// Some No-Intro/Redump markets have no entry in that service's region list.
pub(super) const MARKETS: &[Market] = &[
    market("Asia", "asia", Some("asi")),
    market("Australia", "au", Some("au")),
    market("Austria", "at", None),
    market("Belgium", "be", None),
    market("Canada", "ca", Some("ca")),
    market("China", "cn", Some("cn")),
    market("Denmark", "dk", Some("dk")),
    market("Europe", "eu", Some("eu")),
    market("Finland", "fi", Some("fi")),
    market("France", "fr", Some("fr")),
    market("Germany", "de", Some("de")),
    market("Greece", "gr", Some("gr")),
    market("Ireland", "ie", None),
    market("Israel", "il", Some("il")),
    market("Italy", "it", Some("it")),
    market("Japan", "jp", Some("jp")),
    market("Korea", "kr", Some("kr")),
    market("Netherlands", "nl", Some("nl")),
    market("Norway", "no", Some("no")),
    market("Poland", "pl", Some("pl")),
    market("Portugal", "pt", Some("pt")),
    market("Russia", "ru", Some("ru")),
    market("Scandinavia", "scandinavia", None),
    market("Singapore", "sg", None),
    market("Spain", "es", Some("sp")),
    market("Sweden", "se", Some("se")),
    market("Taiwan", "tw", Some("tw")),
    market("UK", "gb", Some("uk")),
    market("USA", "us", Some("us")),
    market("World", "world", Some("wor")),
];

impl Market {
    pub(super) fn requested(&self, region: &str) -> bool {
        region.split([',', '/']).map(str::trim).any(|part| {
            self.name.eq_ignore_ascii_case(part) || self.identifier.eq_ignore_ascii_case(part)
        })
    }
}
