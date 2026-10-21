//! CatTube's screens and navigation (a pure state machine, like the
//! shell's): input goes to [`Tube::handle`], which returns [`Effect`]s the
//! app performs (requests, playback) and feeds back with the `on_*`
//! methods. Drawing lives in the app's `ui/tube.rs`.
//!
//! ```text
//! Browse     tabs: Home · Subscriptions · History · Playlists · Channels
//!            · Search, rows of tiles under them (Up from the first row:
//!            the tabs, then the account button = Settings)
//! Page       a channel or a playlist, rows of tiles
//! Search     keyboard and suggestions, then result rows
//! Menu       a video's options: Play, Start over, Go to channel,
//!            Like / Dislike, Subscribe, Watch later
//! SignIn     the code to enter at google.com/device
//! TvCode     the code for the phone's "Link with TV code" (phone remote)
//! Settings   accounts, quality, playback, SponsorBlock
//! ```

use crate::input::Action;
use crate::osk::{Edge, Osk, OskEvent};
use crate::scan::Scan;
use crate::youtube::{Details, Kind, Page, Rating, Shelf, Tile, feeds};

/// The tabs, in order. Signed out, only Home (trending-ish results from
/// YouTube's own nudge) and Search work; the others ask to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Home,
    Subscriptions,
    History,
    Playlists,
    Channels,
    Search,
}

impl Section {
    pub const ALL: [Section; 6] = [
        Section::Home,
        Section::Subscriptions,
        Section::History,
        Section::Playlists,
        Section::Channels,
        Section::Search,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Home => "Home",
            Self::Subscriptions => "Subscriptions",
            Self::History => "History",
            Self::Playlists => "Playlists",
            Self::Channels => "Channels",
            Self::Search => "Search",
        }
    }

    pub fn browse_id(self) -> Option<&'static str> {
        match self {
            Self::Home => Some(feeds::HOME),
            Self::Subscriptions => Some(feeds::SUBSCRIPTIONS),
            Self::History => Some(feeds::HISTORY),
            Self::Playlists => Some(feeds::LIBRARY),
            Self::Channels => Some(feeds::CHANNELS),
            Self::Search => None,
        }
    }

    pub fn needs_sign_in(self) -> bool {
        !matches!(self, Self::Home | Self::Search)
    }
}

/// What the video menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItem {
    Play,
    StartOver,
    Channel,
    Like,
    Dislike,
    Subscribe,
    WatchLater,
}

impl MenuItem {
    pub fn label(self, subscribed: bool) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::StartOver => "Start over",
            Self::Channel => "Go to channel",
            Self::Like => "Like",
            Self::Dislike => "Dislike",
            Self::Subscribe if subscribed => "Unsubscribe",
            Self::Subscribe => "Subscribe",
            Self::WatchLater => "Save to Watch later",
        }
    }
}

/// The menu's entries for a tile (signed in or not; resumable or not).
pub fn menu_items(tile: &Tile, signed_in: bool, resumable: bool) -> Vec<MenuItem> {
    let mut v = vec![MenuItem::Play];
    if resumable {
        v.push(MenuItem::StartOver);
    }
    v.push(MenuItem::Channel);
    if signed_in && tile.playable() {
        v.extend([
            MenuItem::Like,
            MenuItem::Dislike,
            MenuItem::Subscribe,
            MenuItem::WatchLater,
        ]);
    }
    v
}

/// Settings rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingRow {
    /// Account `i` (OK uses it, Options signs it out).
    Account(usize),
    AddAccount,
    Quality,
    Codec,
    Hdr,
    Speed,
    Subtitles,
    Autoplay,
    /// Save what plays to the account's YouTube history.
    History,
    Buffer,
    /// The country recommendations are for.
    Region,
    SponsorBlock,
    /// One SponsorBlock category (index into `community::CATEGORIES`).
    Category(usize),
    /// The phone remote: shows the "Link with TV code" code.
    LinkPhone,
    UpdateYtDlp,
}

/// Settings are shown by group: the groups in a list on the left, the
/// focused group's rows on the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingGroup {
    Accounts,
    Video,
    Playback,
    SponsorBlock,
    Phone,
    About,
}

impl SettingGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Accounts => "Accounts",
            Self::Video => "Video",
            Self::Playback => "Playback",
            Self::SponsorBlock => "SponsorBlock",
            Self::Phone => "Phone remote",
            Self::About => "About",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Accounts => "cattube",
            Self::Video => "display",
            Self::Playback => "play",
            Self::SponsorBlock => "check",
            Self::Phone => "phone",
            Self::About => "download",
        }
    }
}

pub fn group_of(row: SettingRow) -> SettingGroup {
    match row {
        SettingRow::Account(_) | SettingRow::AddAccount => SettingGroup::Accounts,
        SettingRow::Quality | SettingRow::Codec | SettingRow::Hdr => SettingGroup::Video,
        SettingRow::Speed
        | SettingRow::Subtitles
        | SettingRow::Autoplay
        | SettingRow::History
        | SettingRow::Buffer
        | SettingRow::Region => SettingGroup::Playback,
        SettingRow::SponsorBlock | SettingRow::Category(_) => SettingGroup::SponsorBlock,
        SettingRow::LinkPhone => SettingGroup::Phone,
        SettingRow::UpdateYtDlp => SettingGroup::About,
    }
}

/// The groups in order, each with the range of its rows in `rows`.
pub fn setting_groups(rows: &[SettingRow]) -> Vec<(SettingGroup, std::ops::Range<usize>)> {
    let mut out: Vec<(SettingGroup, std::ops::Range<usize>)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let g = group_of(*r);
        match out.last_mut() {
            Some((last, range)) if *last == g => range.end = i + 1,
            _ => out.push((g, i..i + 1)),
        }
    }
    out
}

pub fn setting_rows(accounts: usize, sponsorblock: bool) -> Vec<SettingRow> {
    let mut v: Vec<SettingRow> = (0..accounts).map(SettingRow::Account).collect();
    v.extend([
        SettingRow::AddAccount,
        SettingRow::Quality,
        SettingRow::Codec,
        SettingRow::Hdr,
        SettingRow::Speed,
        SettingRow::Subtitles,
        SettingRow::Autoplay,
        SettingRow::History,
        SettingRow::Buffer,
        SettingRow::Region,
        SettingRow::SponsorBlock,
    ]);
    if sponsorblock {
        v.extend((0..crate::community::CATEGORIES.len()).map(SettingRow::Category));
    }
    v.push(SettingRow::LinkPhone);
    v.push(SettingRow::UpdateYtDlp);
    v
}

#[derive(Debug, Clone)]
pub enum Screen {
    Browse,
    /// A channel or playlist (browse id `UC…` / `VL…`).
    Page {
        browse_id: String,
        page: Scan<Page>,
        row: usize,
        cols: Vec<usize>,
    },
    Search {
        osk: Box<Osk>,
        suggestions: Vec<String>,
        /// Focus in the suggestion list (`None`: the keyboard).
        suggestion: Option<usize>,
        /// The results as they are shown ([`search_layout`]).
        results: Scan<Page>,
        /// Every result so far, in YouTube's order (more are added as
        /// you scroll down), and the token for the next ones.
        found: Vec<Tile>,
        more: Option<String>,
        /// More results are on their way.
        loading_more: bool,
        /// Results have focus (the keyboard is put away).
        in_results: bool,
        row: usize,
        cols: Vec<usize>,
    },
    Menu {
        tile: Tile,
        details: Scan<Details>,
        index: usize,
    },
    SignIn {
        code: Scan<(String, String)>,
    },
    /// The 12-digit code (shown "123 456 789 012").
    TvCode {
        code: Scan<String>,
    },
    Settings {
        /// The focused row (in `setting_rows`); its group is the one shown.
        index: usize,
        /// Focus is on the list of groups (else on the group's rows).
        groups: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Load(String),
    /// More rows of a page.
    LoadMore {
        browse_id: String,
        token: String,
    },
    Search(String),
    /// The next results of the search on screen.
    SearchMore(String),
    Suggest(String),
    Details(String),
    Play {
        tile: Box<Tile>,
        from_start: bool,
    },
    SignInStart,
    SignInStop,
    /// Fetch a code for the phone's "Link with TV code".
    TvCode,
    SwitchAccount(usize),
    SignOut(usize),
    Rate(String, Rating),
    Subscribe {
        channel_id: String,
        on: bool,
    },
    WatchLater(String),
    /// A setting row changed by `delta` (−1, +1; OK = +1).
    Setting(SettingRow, i32),
    Close,
    Toast(String),
}

/// What the app knows that the screens need.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ctx {
    pub signed_in: bool,
    pub accounts: usize,
    pub sponsorblock: bool,
}

#[derive(Debug, Clone)]
pub struct Tube {
    pub stack: Vec<Screen>,
    pub section: Section,
    /// Each tab's page (by browse id).
    pub pages: std::collections::HashMap<String, Scan<Page>>,
    /// Browse focus: row and each row's column; or the tab bar.
    pub row: usize,
    pub cols: Vec<usize>,
    pub header: bool,
    /// Which pill has focus in the header: a tab, or `ALL.len()` = account.
    pub header_index: usize,
    pub error: Option<String>,
}

impl Tube {
    pub fn open(ctx: Ctx) -> (Self, Vec<Effect>) {
        let mut t = Tube {
            stack: vec![Screen::Browse],
            section: Section::Home,
            pages: Default::default(),
            row: 0,
            cols: Vec::new(),
            header: false,
            header_index: 0,
            error: None,
        };
        // Signed out, Home is nearly empty on YouTube's TV app: start in
        // Search's place on Home with its results instead.
        let eff = t.select(Section::Home, ctx);
        (t, eff)
    }

    pub fn screen(&self) -> Option<&Screen> {
        self.stack.last()
    }

    /// Typing goes to a keyboard.
    pub fn text_mode(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(Screen::Search {
                in_results: false,
                suggestion: None,
                ..
            })
        )
    }

    /// The page the Browse screen shows.
    pub fn current(&self) -> Option<&Scan<Page>> {
        self.section.browse_id().and_then(|b| self.pages.get(b))
    }

    fn select(&mut self, s: Section, ctx: Ctx) -> Vec<Effect> {
        self.section = s;
        self.row = 0;
        self.cols.clear();
        self.error = None;
        if s == Section::Search {
            // The keyboard has focus, not the top bar.
            self.header = false;
            self.stack.push(search_screen());
            return vec![];
        }
        if s.needs_sign_in() && !ctx.signed_in {
            self.header = true;
            return vec![];
        }
        let id = s.browse_id().unwrap_or_default().to_string();
        match self.pages.get(&id) {
            Some(Scan::Done(_)) | Some(Scan::Busy) => vec![],
            _ => {
                self.pages.insert(id.clone(), Scan::Busy);
                vec![Effect::Load(id)]
            }
        }
    }

    /// Reload the current tab (after signing in or out, after playback).
    pub fn reload(&mut self, ctx: Ctx) -> Vec<Effect> {
        if let Some(id) = self.section.browse_id() {
            self.pages.remove(id);
        }
        let s = self.section;
        if s == Section::Search {
            return vec![];
        }
        self.select(s, ctx)
    }

    /// Forget every tab (the account changed).
    pub fn reset(&mut self, ctx: Ctx) -> Vec<Effect> {
        self.pages.clear();
        self.stack.truncate(1);
        let s = self.section;
        self.select(
            if s == Section::Search {
                Section::Home
            } else {
                s
            },
            ctx,
        )
    }

    fn back(&mut self) -> Vec<Effect> {
        if self.stack.len() > 1 {
            let leaving_sign_in = matches!(self.stack.last(), Some(Screen::SignIn { .. }));
            self.stack.pop();
            self.error = None;
            if matches!(self.stack.last(), Some(Screen::Browse)) && self.section == Section::Search
            {
                self.section = Section::Home;
            }
            return if leaving_sign_in {
                vec![Effect::SignInStop]
            } else {
                vec![]
            };
        }
        if self.header {
            return vec![Effect::Close];
        }
        // Back on a row: up to the tabs first, then out.
        self.header = true;
        self.header_index = Section::ALL
            .iter()
            .position(|s| *s == self.section)
            .unwrap_or(0);
        vec![]
    }

    /// Open a tile: videos play, channels and playlists open their page.
    fn open_tile(&mut self, tile: Tile) -> Vec<Effect> {
        if tile.playable() {
            return vec![Effect::Play {
                tile: Box::new(tile),
                from_start: false,
            }];
        }
        match tile.browse_id() {
            Some(id) => self.open_page(id),
            None => vec![],
        }
    }

    pub fn open_page(&mut self, browse_id: String) -> Vec<Effect> {
        self.stack.push(Screen::Page {
            browse_id: browse_id.clone(),
            page: Scan::Busy,
            row: 0,
            cols: Vec::new(),
        });
        vec![Effect::Load(browse_id)]
    }

    fn open_menu(&mut self, tile: Tile) -> Vec<Effect> {
        let id = tile.id.clone();
        let playable = tile.playable();
        self.stack.push(Screen::Menu {
            tile,
            details: if playable { Scan::Busy } else { Scan::Idle },
            index: 0,
        });
        if playable {
            vec![Effect::Details(id)]
        } else {
            vec![]
        }
    }

    pub fn sign_in(&mut self) -> Vec<Effect> {
        self.error = None;
        self.stack.push(Screen::SignIn { code: Scan::Busy });
        vec![Effect::SignInStart]
    }

    pub fn handle(
        &mut self,
        action: Action,
        ctx: Ctx,
        resumable: &dyn Fn(&str) -> bool,
    ) -> Vec<Effect> {
        // The top bar over Search (Up from the search field).
        if self.header && matches!(self.stack.last(), Some(Screen::Search { .. })) {
            return self.handle_search_header(action, ctx);
        }
        let Some(screen) = self.stack.last_mut() else {
            return vec![Effect::Close];
        };
        match screen {
            Screen::Browse => self.handle_browse(action, ctx),
            Screen::Page {
                page,
                row,
                cols,
                browse_id,
            } => {
                let pg = page.items().first().cloned();
                let id = browse_id.clone();
                match action {
                    Action::Back => self.back(),
                    Action::Ok if matches!(page, Scan::Failed(_)) => {
                        *page = Scan::Busy;
                        vec![Effect::Load(id)]
                    }
                    a => {
                        let Some(p) = pg else { return vec![] };
                        match rows_move(&p, row, cols, a) {
                            RowMove::Open(t) => self.open_tile(t),
                            RowMove::Menu(t) => self.open_menu(t),
                            RowMove::More(tok) => vec![Effect::LoadMore {
                                browse_id: id,
                                token: tok,
                            }],
                            RowMove::Up | RowMove::None => vec![],
                        }
                    }
                }
            }
            Screen::Search {
                osk,
                suggestions,
                suggestion,
                results,
                loading_more,
                in_results,
                row,
                cols,
                ..
            } => {
                if *in_results {
                    let Some(p) = results.items().first().cloned() else {
                        *in_results = false;
                        return vec![];
                    };
                    if action == Action::Back {
                        *in_results = false;
                        return vec![];
                    }
                    // Up and down the grid of videos, the column stays.
                    let from = (*row, cols.get(*row).copied().unwrap_or(0));
                    let mv = rows_move(&p, row, cols, action);
                    let grid =
                        |r: usize| p.shelves.get(r).is_some_and(|sh| sh.title != CHANNELS_ROW);
                    if *row != from.0 && grid(*row) && grid(from.0) {
                        let len = p.shelves[*row].tiles.len();
                        cols[*row] = from.1.min(len.saturating_sub(1));
                    }
                    return match mv {
                        RowMove::Open(t) => self.open_tile(t),
                        RowMove::Menu(t) => self.open_menu(t),
                        RowMove::Up => {
                            if let Some(Screen::Search { in_results, .. }) = self.stack.last_mut() {
                                *in_results = false;
                            }
                            vec![]
                        }
                        RowMove::More(t) if !*loading_more => {
                            *loading_more = true;
                            vec![Effect::SearchMore(t)]
                        }
                        RowMove::More(_) | RowMove::None => vec![],
                    };
                }
                if let Some(i) = *suggestion {
                    match action {
                        Action::Up if i == 0 => *suggestion = None,
                        Action::Up => *suggestion = Some(i - 1),
                        Action::Down if i + 1 < suggestions.len() => *suggestion = Some(i + 1),
                        Action::Down if !results.items().is_empty() => {
                            *suggestion = None;
                            *in_results = true;
                        }
                        Action::Left | Action::Back => *suggestion = None,
                        Action::Ok | Action::OkLong => {
                            let q = suggestions[i].clone();
                            osk.set_text(&q);
                            *suggestion = None;
                            *results = Scan::Busy;
                            *row = 0;
                            cols.clear();
                            *loading_more = false;
                            return vec![Effect::Search(q)];
                        }
                        _ => {}
                    }
                    return vec![];
                }
                let before = osk.text().to_string();
                let edge = osk.handle(action);
                // Up from the search field: the top bar.
                if edge == OskEvent::Edge(Edge::Up) {
                    self.header = true;
                    self.header_index = search_tab();
                    return vec![];
                }
                // Right off the keyboard: the suggestions; Down off it:
                // the results.
                match edge {
                    OskEvent::Edge(Edge::Right) if !suggestions.is_empty() => {
                        *suggestion = Some(0);
                        return vec![];
                    }
                    OskEvent::Edge(Edge::Down) if !results.items().is_empty() => {
                        *in_results = true;
                        return vec![];
                    }
                    _ => {}
                }
                match edge {
                    OskEvent::Submitted => {
                        let q = osk.text().trim().to_string();
                        if q.is_empty() {
                            return vec![];
                        }
                        *results = Scan::Busy;
                        *row = 0;
                        cols.clear();
                        *loading_more = false;
                        vec![Effect::Search(q)]
                    }
                    OskEvent::Unhandled if action == Action::Back => self.back(),
                    _ => {
                        let now = osk.text().to_string();
                        if now != before && now.trim().len() >= 2 {
                            vec![Effect::Suggest(now)]
                        } else {
                            if now.trim().len() < 2 {
                                suggestions.clear();
                            }
                            vec![]
                        }
                    }
                }
            }
            Screen::Menu {
                tile,
                details,
                index,
            } => {
                let subscribed = details.items().first().is_some_and(|d| d.subscribed);
                let items = menu_items(tile, ctx.signed_in, resumable(&tile.id));
                match action {
                    Action::Back => self.back(),
                    Action::Up => {
                        *index = index.saturating_sub(1);
                        vec![]
                    }
                    Action::Down => {
                        *index = (*index + 1).min(items.len().saturating_sub(1));
                        vec![]
                    }
                    Action::Ok | Action::OkLong => {
                        let tile = tile.clone();
                        let channel = details
                            .items()
                            .first()
                            .map(|d| d.channel_id.clone())
                            .filter(|c| !c.is_empty())
                            .unwrap_or_else(|| tile.channel_id.clone());
                        let item = items.get(*index).copied();
                        self.stack.pop();
                        match item {
                            Some(MenuItem::Play) => vec![Effect::Play {
                                tile: Box::new(tile),
                                from_start: false,
                            }],
                            Some(MenuItem::StartOver) => vec![Effect::Play {
                                tile: Box::new(tile),
                                from_start: true,
                            }],
                            Some(MenuItem::Channel) if !channel.is_empty() => {
                                self.open_page(channel)
                            }
                            Some(MenuItem::Channel) => {
                                vec![Effect::Toast("No channel for this one".into())]
                            }
                            Some(MenuItem::Like) => vec![Effect::Rate(tile.id, Rating::Like)],
                            Some(MenuItem::Dislike) => vec![Effect::Rate(tile.id, Rating::Dislike)],
                            Some(MenuItem::Subscribe) if !channel.is_empty() => {
                                vec![Effect::Subscribe {
                                    channel_id: channel,
                                    on: !subscribed,
                                }]
                            }
                            Some(MenuItem::WatchLater) => vec![Effect::WatchLater(tile.id)],
                            _ => vec![],
                        }
                    }
                    _ => vec![],
                }
            }
            Screen::TvCode { code } => match action {
                Action::Back => self.back(),
                Action::Ok => {
                    *code = Scan::Busy;
                    vec![Effect::TvCode]
                }
                _ => vec![],
            },
            Screen::SignIn { code } => match action {
                Action::Back => self.back(),
                Action::Ok if matches!(code, Scan::Failed(_)) => {
                    *code = Scan::Busy;
                    self.error = None;
                    vec![Effect::SignInStop, Effect::SignInStart]
                }
                _ => vec![],
            },
            Screen::Settings { index, groups } => {
                let rows = setting_rows(ctx.accounts, ctx.sponsorblock);
                let row = rows.get(*index).copied();
                let all = setting_groups(&rows);
                let gi = all.iter().position(|(_, r)| r.contains(index)).unwrap_or(0);
                let range = all.get(gi).map_or(0..0, |(_, r)| r.clone());
                if *groups {
                    // The groups: Up / Down pick one (its first row),
                    // Right or OK go into it, Back leaves Settings.
                    match action {
                        Action::Up if gi > 0 => *index = all[gi - 1].1.start,
                        Action::Down if gi + 1 < all.len() => *index = all[gi + 1].1.start,
                        Action::Right | Action::Ok => *groups = false,
                        Action::Back => return self.back(),
                        _ => {}
                    }
                    return vec![];
                }
                // Back, or Left on a row that has no value to change: the
                // groups again.
                let to_groups = matches!(
                    (action, row),
                    (Action::Back, _)
                        | (
                            Action::Left,
                            Some(SettingRow::Account(_))
                                | Some(SettingRow::AddAccount)
                                | Some(SettingRow::LinkPhone)
                                | Some(SettingRow::UpdateYtDlp)
                                | Some(SettingRow::Hdr)
                                | Some(SettingRow::Subtitles)
                                | Some(SettingRow::History)
                                | Some(SettingRow::SponsorBlock)
                                | Some(SettingRow::Category(_))
                                | None,
                        )
                );
                if to_groups {
                    *groups = true;
                    return vec![];
                }
                match (action, row) {
                    (Action::Up, _) => {
                        if *index > range.start {
                            *index -= 1;
                        }
                        vec![]
                    }
                    (Action::Down, _) => {
                        if *index + 1 < range.end {
                            *index += 1;
                        }
                        vec![]
                    }
                    (Action::Ok, Some(SettingRow::Account(i))) => {
                        let eff = vec![Effect::SwitchAccount(i)];
                        self.stack.truncate(1);
                        eff
                    }
                    (Action::OkLong | Action::Options, Some(SettingRow::Account(i))) => {
                        *index = index.saturating_sub(1);
                        vec![Effect::SignOut(i)]
                    }
                    (Action::Ok, Some(SettingRow::AddAccount)) => self.sign_in(),
                    (Action::Ok, Some(SettingRow::LinkPhone)) => {
                        self.stack.push(Screen::TvCode { code: Scan::Busy });
                        vec![Effect::TvCode]
                    }
                    (Action::Ok | Action::Right, Some(r)) => vec![Effect::Setting(r, 1)],
                    (Action::Left, Some(r)) => vec![Effect::Setting(r, -1)],
                    _ => vec![],
                }
            }
        }
    }

    /// The top bar while Search is open: Left / Right along it, Down (or
    /// Back) back to the search, OK on another tab leaves Search for it.
    fn handle_search_header(&mut self, action: Action, ctx: Ctx) -> Vec<Effect> {
        let n_tabs = Section::ALL.len();
        match action {
            Action::Left => self.header_index = self.header_index.saturating_sub(1),
            Action::Right => self.header_index = (self.header_index + 1).min(n_tabs),
            Action::Down | Action::Back => self.header = false,
            Action::Ok | Action::OkLong => {
                if self.header_index == search_tab() {
                    self.header = false;
                    return vec![];
                }
                // Leave Search; the Browse screen's top bar takes the OK.
                self.stack.pop();
                self.section = Section::Home;
                return self.handle_browse(Action::Ok, ctx);
            }
            _ => {}
        }
        vec![]
    }

    fn handle_browse(&mut self, action: Action, ctx: Ctx) -> Vec<Effect> {
        let n_tabs = Section::ALL.len();
        if self.header {
            match action {
                Action::Left => self.header_index = self.header_index.saturating_sub(1),
                Action::Right => self.header_index = (self.header_index + 1).min(n_tabs),
                Action::Down => {
                    let has_rows = self
                        .current()
                        .and_then(|p| p.items().first())
                        .is_some_and(|p| !p.shelves.is_empty());
                    if has_rows {
                        self.header = false;
                    }
                }
                Action::Ok | Action::OkLong => {
                    if self.header_index == n_tabs {
                        self.stack.push(Screen::Settings {
                            index: 0,
                            groups: true,
                        });
                        return vec![];
                    }
                    let s = Section::ALL[self.header_index];
                    if s.needs_sign_in() && !ctx.signed_in {
                        self.section = s;
                        return self.sign_in();
                    }
                    let eff = self.select(s, ctx);
                    if s != Section::Search {
                        self.header = !self
                            .current()
                            .and_then(|p| p.items().first())
                            .is_some_and(|p| !p.shelves.is_empty());
                    }
                    return eff;
                }
                Action::Back => return self.back(),
                _ => {}
            }
            return vec![];
        }
        let Some(page) = self.current().and_then(|p| p.items().first()).cloned() else {
            match action {
                Action::Up | Action::Back => self.header = true,
                Action::Ok if matches!(self.current(), Some(Scan::Failed(_))) => {
                    return self.reload(ctx);
                }
                _ => {}
            }
            return vec![];
        };
        if action == Action::Back {
            return self.back();
        }
        let id = self.section.browse_id().unwrap_or_default().to_string();
        let (mut row, mut cols) = (self.row, std::mem::take(&mut self.cols));
        let m = rows_move(&page, &mut row, &mut cols, action);
        self.row = row;
        self.cols = cols;
        match m {
            RowMove::Up => {
                self.header = true;
                self.header_index = Section::ALL
                    .iter()
                    .position(|s| *s == self.section)
                    .unwrap_or(0);
                vec![]
            }
            RowMove::Open(t) => self.open_tile(t),
            RowMove::Menu(t) => self.open_menu(t),
            RowMove::More(tok) => vec![Effect::LoadMore {
                browse_id: id,
                token: tok,
            }],
            RowMove::None => vec![],
        }
    }

    /// The tile with focus on the current screen.
    pub fn focused(&self) -> Option<Tile> {
        match self.stack.last()? {
            Screen::Browse if !self.header => {
                let p = self.current()?.items().first()?;
                tile_at(p, self.row, &self.cols)
            }
            Screen::Page {
                page, row, cols, ..
            } => tile_at(page.items().first()?, *row, cols),
            Screen::Search {
                results,
                in_results: true,
                row,
                cols,
                ..
            } => tile_at(results.items().first()?, *row, cols),
            Screen::Menu { tile, .. } => Some(tile.clone()),
            _ => None,
        }
    }

    // ------------------------------------------------------------ results

    /// A page (a tab's, or a channel's / playlist's) arrived.
    pub fn on_page(&mut self, browse_id: &str, result: Result<Page, String>) {
        let scan = match result {
            Ok(p) => Scan::Done(vec![p]),
            Err(e) => Scan::Failed(e),
        };
        for s in &mut self.stack {
            if let Screen::Page {
                browse_id: b, page, ..
            } = s
                && b == browse_id
            {
                *page = scan.clone();
            }
        }
        if Section::ALL
            .iter()
            .any(|s| s.browse_id() == Some(browse_id))
        {
            let empty = scan.items().first().is_none_or(|p| p.shelves.is_empty());
            self.pages.insert(browse_id.to_string(), scan);
            if self.section.browse_id() == Some(browse_id) && empty {
                self.header = true;
            }
        }
    }

    /// More rows (or a longer row) for a page.
    pub fn on_more(&mut self, browse_id: &str, token: &str, more: Page) {
        let merge = |p: &mut Page| {
            // A row's own continuation grows that row; a page's adds rows.
            if let Some(sh) = p
                .shelves
                .iter_mut()
                .find(|s| s.continuation.as_deref() == Some(token))
            {
                for t in more.tiles() {
                    if !sh.tiles.iter().any(|x| x.id == t.id) {
                        sh.tiles.push(t);
                    }
                }
                sh.continuation = more
                    .continuation
                    .clone()
                    .or_else(|| more.shelves.first().and_then(|s| s.continuation.clone()));
            } else if p.continuation.as_deref() == Some(token) {
                if p.shelves.len() == 1 && more.shelves.iter().all(|s| s.title.is_empty()) {
                    let sh = &mut p.shelves[0];
                    for t in more.tiles() {
                        if !sh.tiles.iter().any(|x| x.id == t.id) {
                            sh.tiles.push(t);
                        }
                    }
                } else {
                    p.shelves.extend(more.shelves.clone());
                }
                p.continuation = more.continuation.clone();
            }
        };
        if let Some(Scan::Done(v)) = self.pages.get_mut(browse_id)
            && let Some(p) = v.first_mut()
        {
            merge(p);
        }
        for s in &mut self.stack {
            if let Screen::Page {
                browse_id: b,
                page: Scan::Done(v),
                ..
            } = s
                && b == browse_id
                && let Some(p) = v.first_mut()
            {
                merge(p);
            }
        }
    }

    pub fn on_search(&mut self, result: Result<Page, String>) {
        if let Some(Screen::Search {
            results,
            found,
            more,
            loading_more,
            in_results,
            row,
            cols,
            ..
        }) = self.stack.last_mut()
        {
            *loading_more = false;
            *row = 0;
            cols.clear();
            match result {
                Ok(p) => {
                    let (tiles, next) = search_results(&p);
                    *found = tiles;
                    *more = next;
                    let shown = search_layout(found, more.clone());
                    *in_results = !shown.shelves.is_empty();
                    *results = Scan::Done(vec![shown]);
                }
                Err(e) => {
                    found.clear();
                    *more = None;
                    *results = Scan::Failed(e);
                    *in_results = false;
                }
            }
        }
    }

    /// The next results arrived (scrolling down): added at the end.
    pub fn on_search_more(&mut self, result: Result<Page, String>) {
        if let Some(Screen::Search {
            results,
            found,
            more,
            loading_more,
            ..
        }) = self.stack.last_mut()
        {
            *loading_more = false;
            let Ok(p) = result else {
                return;
            };
            let (tiles, next) = search_results(&p);
            for t in tiles {
                if !found.iter().any(|f| f.id == t.id) {
                    found.push(t);
                }
            }
            *more = next;
            *results = Scan::Done(vec![search_layout(found, more.clone())]);
        }
    }

    pub fn on_suggestions(&mut self, query: &str, list: Vec<String>) {
        if let Some(Screen::Search {
            osk, suggestions, ..
        }) = self.stack.last_mut()
            && osk.text() == query
        {
            *suggestions = list;
        }
    }

    pub fn on_details(&mut self, video_id: &str, result: Result<Details, String>) {
        for s in &mut self.stack {
            if let Screen::Menu { tile, details, .. } = s
                && tile.id == video_id
            {
                *details = match &result {
                    Ok(d) => Scan::Done(vec![d.clone()]),
                    Err(e) => Scan::Failed(e.clone()),
                };
            }
        }
    }

    /// The sign-in code to show, or why there is none.
    pub fn on_sign_in_code(&mut self, result: Result<(String, String), String>) {
        if let Some(Screen::SignIn { code }) = self.stack.last_mut() {
            *code = match result {
                Ok(c) => Scan::Done(vec![c]),
                Err(e) => Scan::Failed(e),
            };
        }
    }

    /// The phone code to show (a new one replaces the old).
    pub fn on_tv_code(&mut self, result: Result<String, String>) {
        if let Some(Screen::TvCode { code }) = self.stack.last_mut() {
            *code = match result {
                Ok(c) => Scan::Done(vec![c]),
                Err(e) => Scan::Failed(e),
            };
        }
    }

    /// Signed in (or it failed for good).
    pub fn on_signed_in(&mut self, result: Result<(), String>, ctx: Ctx) -> Vec<Effect> {
        match result {
            Ok(()) => {
                self.stack.truncate(1);
                self.header = false;
                self.reset(ctx)
            }
            Err(e) => {
                self.on_sign_in_code(Err(e));
                vec![]
            }
        }
    }
}

/// Search's place in the top bar.
fn search_tab() -> usize {
    Section::ALL
        .iter()
        .position(|s| *s == Section::Search)
        .unwrap_or(0)
}

/// Search results show this many videos to a row.
pub const SEARCH_COLS: usize = 4;
/// The title of the channels row above the videos.
pub const CHANNELS_ROW: &str = "Channels";

/// The real results of a search reply, and the token for more: YouTube's
/// own results row only ("Search results for …"); the rows it adds
/// ("People also watched", "Shorts", "For you") and Shorts are left out.
pub fn search_results(p: &Page) -> (Vec<Tile>, Option<String>) {
    let main = p
        .shelves
        .iter()
        .find(|sh| sh.tiles.iter().any(|t| t.kind != Kind::Short))
        .or(p.shelves.first());
    let Some(main) = main else {
        return (Vec::new(), None);
    };
    let tiles = main
        .tiles
        .iter()
        .filter(|t| t.kind != Kind::Short)
        .cloned()
        .collect();
    (tiles, main.continuation.clone().or(p.continuation.clone()))
}

/// Results as rows: the channels first (one row), then the videos and
/// playlists [`SEARCH_COLS`] to a row. `more` is the page's continuation,
/// so scrolling to the last rows asks for the next results.
pub fn search_layout(found: &[Tile], more: Option<String>) -> Page {
    let (channels, videos): (Vec<Tile>, Vec<Tile>) =
        found.iter().cloned().partition(|t| t.kind == Kind::Channel);
    let mut shelves = Vec::new();
    if !channels.is_empty() {
        shelves.push(Shelf {
            title: CHANNELS_ROW.into(),
            tiles: channels,
            ..Shelf::default()
        });
    }
    for (i, chunk) in videos.chunks(SEARCH_COLS).enumerate() {
        shelves.push(Shelf {
            title: if i == 0 {
                "Videos".into()
            } else {
                String::new()
            },
            tiles: chunk.to_vec(),
            ..Shelf::default()
        });
    }
    Page {
        title: String::new(),
        shelves,
        continuation: more,
    }
}

fn search_screen() -> Screen {
    Screen::Search {
        osk: Box::new(Osk::new(false, "Search")),
        suggestions: Vec::new(),
        suggestion: None,
        results: Scan::Idle,
        found: Vec::new(),
        more: None,
        loading_more: false,
        in_results: false,
        row: 0,
        cols: Vec::new(),
    }
}

fn tile_at(p: &Page, row: usize, cols: &[usize]) -> Option<Tile> {
    let r = p.shelves.get(row)?;
    r.tiles.get(cols.get(row).copied().unwrap_or(0)).cloned()
}

enum RowMove {
    None,
    /// Up from the first row.
    Up,
    Open(Tile),
    Menu(Tile),
    /// Near the end of a row (or the last row) with more to fetch.
    More(String),
}

/// Rows of tiles: Up / Down between rows, Left / Right along one, OK opens,
/// holding OK (or Options) opens the menu.
fn rows_move(p: &Page, row: &mut usize, cols: &mut Vec<usize>, a: Action) -> RowMove {
    let n = p.shelves.len();
    if n == 0 {
        return if a == Action::Up {
            RowMove::Up
        } else {
            RowMove::None
        };
    }
    *row = (*row).min(n - 1);
    if cols.len() < n {
        cols.resize(n, 0);
    }
    let len = p.shelves[*row].tiles.len();
    let col = cols[*row].min(len.saturating_sub(1));
    match a {
        Action::Up if *row == 0 => RowMove::Up,
        Action::Up => {
            *row -= 1;
            RowMove::None
        }
        Action::Down if *row + 1 < n => {
            *row += 1;
            // The page's last rows: fetch more rows.
            match (&p.continuation, *row + 2 >= n) {
                (Some(t), true) => RowMove::More(t.clone()),
                _ => RowMove::None,
            }
        }
        Action::Down => p.continuation.clone().map_or(RowMove::None, RowMove::More),
        Action::Left => {
            cols[*row] = col.saturating_sub(1);
            RowMove::None
        }
        Action::Right => {
            cols[*row] = (col + 1).min(len.saturating_sub(1));
            let shelf = &p.shelves[*row];
            match &shelf.continuation {
                Some(t) if cols[*row] + 4 >= len => RowMove::More(t.clone()),
                // One row (a playlist): the page's continuation grows it.
                None if n == 1 && cols[*row] + 4 >= len => {
                    p.continuation.clone().map_or(RowMove::None, RowMove::More)
                }
                _ => RowMove::None,
            }
        }
        Action::Ok => p.shelves[*row]
            .tiles
            .get(col)
            .cloned()
            .map_or(RowMove::None, RowMove::Open),
        Action::OkLong | Action::Options => p.shelves[*row]
            .tiles
            .get(col)
            .cloned()
            .map_or(RowMove::None, RowMove::Menu),
        _ => RowMove::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Action::*;
    use crate::youtube::{Kind, Shelf};

    fn tile(id: &str, kind: Kind) -> Tile {
        Tile {
            kind,
            id: id.into(),
            title: id.into(),
            channel_id: "UCabc".into(),
            ..Tile::default()
        }
    }

    fn page(rows: &[&[&str]]) -> Page {
        Page {
            shelves: rows
                .iter()
                .enumerate()
                .map(|(i, ids)| Shelf {
                    title: format!("Row {i}"),
                    tiles: ids.iter().map(|id| tile(id, Kind::Video)).collect(),
                    ..Shelf::default()
                })
                .collect(),
            ..Page::default()
        }
    }

    const OUT: Ctx = Ctx {
        signed_in: false,
        accounts: 0,
        sponsorblock: true,
    };
    const IN: Ctx = Ctx {
        signed_in: true,
        accounts: 1,
        sponsorblock: true,
    };
    fn no(_: &str) -> bool {
        false
    }

    fn ok<T>(v: T) -> Result<T, String> {
        Result::Ok(v)
    }

    #[test]
    fn opens_on_home_and_plays() {
        let (mut t, eff) = Tube::open(IN);
        assert_eq!(eff, vec![Effect::Load("FEwhat_to_watch".into())]);
        t.on_page(
            "FEwhat_to_watch",
            ok(page(&[&["aaaaaaaaaaa", "bbbbbbbbbbb"], &["ccccccccccc"]])),
        );
        assert!(!t.header);
        t.handle(Right, IN, &no);
        assert_eq!(t.focused().unwrap().id, "bbbbbbbbbbb");
        let e = t.handle(Ok, IN, &no);
        assert!(
            matches!(&e[0], Effect::Play { tile, from_start: false } if tile.id == "bbbbbbbbbbb")
        );
        // Down then Up keeps each row's place.
        t.handle(Down, IN, &no);
        t.handle(Up, IN, &no);
        assert_eq!(t.focused().unwrap().id, "bbbbbbbbbbb");
        // Up from the first row: the tabs; Back there closes.
        t.handle(Up, IN, &no);
        assert!(t.header);
        assert_eq!(t.handle(Back, IN, &no), vec![Effect::Close]);
    }

    #[test]
    fn tabs_load_once_and_ask_to_sign_in() {
        let (mut t, _) = Tube::open(OUT);
        t.on_page("FEwhat_to_watch", ok(Page::default()));
        assert!(t.header);
        t.handle(Right, OUT, &no);
        // Subscriptions signed out: the sign-in screen.
        assert_eq!(t.handle(Ok, OUT, &no), vec![Effect::SignInStart]);
        t.on_sign_in_code(ok((
            "WZY-WHH-FZQ".into(),
            "https://www.google.com/device".into(),
        )));
        assert_eq!(t.handle(Back, OUT, &no), vec![Effect::SignInStop]);
        // Signed in: Subscriptions loads, and only once.
        let (mut t, _) = Tube::open(IN);
        t.header = true;
        t.header_index = 1;
        assert_eq!(
            t.handle(Ok, IN, &no),
            vec![Effect::Load("FEsubscriptions".into())]
        );
        t.header = true;
        assert!(t.handle(Ok, IN, &no).is_empty());
        // The last pill is the account button: Settings.
        t.header = true;
        t.header_index = Section::ALL.len();
        t.handle(Ok, IN, &no);
        assert!(matches!(t.screen(), Some(Screen::Settings { .. })));
    }

    #[test]
    fn channels_and_playlists_open_pages() {
        let (mut t, _) = Tube::open(IN);
        let mut p = page(&[&[]]);
        p.shelves[0].tiles = vec![tile("UCxyz", Kind::Channel), tile("PLabc", Kind::Playlist)];
        t.on_page("FEwhat_to_watch", ok(p));
        assert_eq!(t.handle(Ok, IN, &no), vec![Effect::Load("UCxyz".into())]);
        t.handle(Back, IN, &no);
        t.handle(Right, IN, &no);
        assert_eq!(t.handle(Ok, IN, &no), vec![Effect::Load("VLPLabc".into())]);
        t.on_page("VLPLabc", ok(page(&[&["ddddddddddd"]])));
        assert!(
            matches!(&t.handle(Ok, IN, &no)[0], Effect::Play { tile, .. } if tile.id == "ddddddddddd")
        );
    }

    #[test]
    fn video_menu() {
        let (mut t, _) = Tube::open(IN);
        t.on_page("FEwhat_to_watch", ok(page(&[&["aaaaaaaaaaa"]])));
        let resumable = |id: &str| id == "aaaaaaaaaaa";
        assert_eq!(
            t.handle(OkLong, IN, &resumable),
            vec![Effect::Details("aaaaaaaaaaa".into())]
        );
        // Play, Start over, Channel, Like, Dislike, Subscribe, Watch later.
        t.handle(Down, IN, &resumable);
        assert!(matches!(
            &t.handle(Ok, IN, &resumable)[0],
            Effect::Play {
                from_start: true,
                ..
            }
        ));
        t.handle(OkLong, IN, &resumable);
        t.on_details(
            "aaaaaaaaaaa",
            ok(Details {
                channel_id: "UCchan".into(),
                subscribed: true,
                ..Details::default()
            }),
        );
        for _ in 0..5 {
            t.handle(Down, IN, &resumable);
        }
        assert_eq!(
            t.handle(Ok, IN, &resumable),
            vec![Effect::Subscribe {
                channel_id: "UCchan".into(),
                on: false
            }]
        );
        // Signed out: no rating or subscribing.
        assert_eq!(
            menu_items(&tile("aaaaaaaaaaa", Kind::Video), false, false),
            vec![MenuItem::Play, MenuItem::Channel]
        );
    }

    #[test]
    fn search_results_are_a_channel_row_and_a_grid_of_videos() {
        let ids: Vec<String> = (0..10).map(|i| format!("v{i:010}")).collect();
        let mut main: Vec<Tile> = ids.iter().map(|i| tile(i, Kind::Video)).collect();
        main.insert(1, tile("UCchannel01", Kind::Channel));
        main.push(tile("shortshort1", Kind::Short));
        let reply = Page {
            shelves: vec![
                Shelf {
                    title: "Search results for cats".into(),
                    tiles: main,
                    continuation: Some("next".into()),
                    ..Shelf::default()
                },
                // YouTube's own extra rows: left out.
                Shelf {
                    title: "Shorts".into(),
                    tiles: vec![tile("shortshort2", Kind::Short)],
                    ..Shelf::default()
                },
                Shelf {
                    title: "People also watched".into(),
                    tiles: vec![tile("othervideo1", Kind::Video)],
                    ..Shelf::default()
                },
            ],
            ..Page::default()
        };
        let (found, more) = search_results(&reply);
        assert_eq!(found.len(), 11, "no Shorts, no other rows");
        assert_eq!(more.as_deref(), Some("next"));
        let p = search_layout(&found, more);
        let titles: Vec<&str> = p.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, [CHANNELS_ROW, "Videos", "", ""]);
        assert_eq!(p.shelves[1].tiles.len(), SEARCH_COLS);
        assert_eq!(p.shelves[3].tiles.len(), 2);
        assert_eq!(p.continuation.as_deref(), Some("next"));

        // On screen: Down from a video keeps its column; near the end the
        // next results are asked for, once.
        let (mut t, _) = Tube::open(IN);
        t.stack.push(search_screen());
        t.on_search(ok(reply));
        assert!(matches!(
            t.screen(),
            Some(Screen::Search {
                in_results: true,
                ..
            })
        ));
        t.handle(Down, IN, &no); // channels → videos
        t.handle(Right, IN, &no);
        t.handle(Right, IN, &no);
        assert_eq!(t.focused().unwrap().id, ids[2]);
        assert_eq!(
            t.handle(Down, IN, &no),
            vec![Effect::SearchMore("next".into())]
        );
        assert_eq!(t.focused().unwrap().id, ids[6], "same column");
        assert!(t.handle(Down, IN, &no).is_empty(), "already loading");
        let more: Vec<Tile> = (10..14)
            .map(|i| tile(&format!("v{i:010}"), Kind::Video))
            .collect();
        t.on_search_more(ok(Page {
            shelves: vec![Shelf {
                tiles: more,
                ..Shelf::default()
            }],
            ..Page::default()
        }));
        if let Some(Screen::Search {
            results,
            found,
            more,
            ..
        }) = t.screen()
        {
            assert_eq!(found.len(), 15);
            assert_eq!(*more, None);
            assert_eq!(results.items()[0].shelves.len(), 5);
        } else {
            panic!();
        }
    }

    #[test]
    fn right_and_down_off_the_keyboard_reach_suggestions_and_results() {
        let (mut t, _) = Tube::open(IN);
        t.stack.push(search_screen());
        if let Some(Screen::Search { osk, .. }) = t.stack.last_mut() {
            osk.set_text("mkbh");
        }
        t.on_suggestions("mkbh", vec!["mkbhd".into(), "mkbhd iphone".into()]);
        // Along the top row of keys to its right end, then one more.
        for _ in 0..12 {
            t.handle(Right, IN, &no);
        }
        assert!(matches!(
            t.screen(),
            Some(Screen::Search {
                suggestion: Some(0),
                ..
            })
        ));
        assert_eq!(t.handle(Ok, IN, &no), vec![Effect::Search("mkbhd".into())]);
        t.on_search(ok(page(&[&["eeeeeeeeeee"]])));
        // Up out of the results, then Down off the keyboard's last row.
        t.handle(Up, IN, &no);
        assert!(t.text_mode());
        for _ in 0..6 {
            t.handle(Down, IN, &no);
        }
        assert!(matches!(
            t.screen(),
            Some(Screen::Search {
                in_results: true,
                ..
            })
        ));
    }

    #[test]
    fn search_with_suggestions() {
        let (mut t, _) = Tube::open(IN);
        t.header = true;
        t.header_index = 5;
        t.handle(Ok, IN, &no);
        assert!(t.text_mode());
        // Typing "ca" (the keyboard's own keys stand in for typed text).
        if let Some(Screen::Search { osk, .. }) = t.stack.last_mut() {
            osk.set_text("ca");
        }
        t.on_suggestions("ca", vec!["cats".into(), "cars".into()]);
        if let Some(Screen::Search { suggestion, .. }) = t.stack.last_mut() {
            *suggestion = Some(1);
        }
        assert_eq!(t.handle(Ok, IN, &no), vec![Effect::Search("cars".into())]);
        t.on_search(ok(page(&[&["eeeeeeeeeee"]])));
        assert_eq!(t.focused().unwrap().id, "eeeeeeeeeee");
        // Back from results: the keyboard; again: out of Search.
        t.handle(Back, IN, &no);
        assert!(t.text_mode());
        t.handle(Back, IN, &no);
        assert!(matches!(t.screen(), Some(Screen::Browse)));
        assert_eq!(t.section, Section::Home);
    }

    #[test]
    fn up_from_search_reaches_the_top_bar() {
        let (mut t, _) = Tube::open(IN);
        t.header = true;
        t.header_index = search_tab();
        t.handle(Ok, IN, &no);
        assert!(!t.header, "the keyboard has focus");
        // Up through the keys, into the field, then the top bar.
        for _ in 0..8 {
            t.handle(Up, IN, &no);
        }
        assert!(t.header);
        assert_eq!(t.header_index, search_tab());
        // Down: back to the search; Up again, Left to History, OK opens it.
        t.handle(Down, IN, &no);
        assert!(!t.header && t.text_mode());
        for _ in 0..8 {
            t.handle(Up, IN, &no);
        }
        t.handle(Left, IN, &no);
        t.handle(Left, IN, &no);
        t.handle(Ok, IN, &no);
        assert!(matches!(t.screen(), Some(Screen::Browse)));
        assert_eq!(t.section, Section::ALL[search_tab() - 2]);
    }

    #[test]
    fn rows_fetch_more_near_the_end() {
        let (mut t, _) = Tube::open(IN);
        let mut p = page(&[&["a0000000000", "a1111111111"]]);
        p.shelves[0].continuation = Some("TOK".into());
        t.on_page("FEwhat_to_watch", ok(p));
        assert_eq!(
            t.handle(Right, IN, &no),
            vec![Effect::LoadMore {
                browse_id: "FEwhat_to_watch".into(),
                token: "TOK".into()
            }]
        );
        let more = page(&[&["a2222222222"]]);
        t.on_more("FEwhat_to_watch", "TOK", more);
        let p = t.current().unwrap().items()[0].clone();
        assert_eq!(p.shelves[0].tiles.len(), 3);
        assert_eq!(p.shelves[0].continuation, None);
    }

    #[test]
    fn settings_rows() {
        assert_eq!(
            setting_rows(2, false)[..3],
            [
                SettingRow::Account(0),
                SettingRow::Account(1),
                SettingRow::AddAccount
            ]
        );
        let all = setting_rows(0, true);
        assert!(all.contains(&SettingRow::Category(0)));
        assert_eq!(*all.last().unwrap(), SettingRow::UpdateYtDlp);
        // The phone code: OK opens it, OK again asks for a fresh one.
        let (mut p, _) = Tube::open(IN);
        let at = setting_rows(OUT.accounts, OUT.sponsorblock)
            .iter()
            .position(|r| *r == SettingRow::LinkPhone)
            .unwrap();
        p.stack.push(Screen::Settings {
            index: at,
            groups: false,
        });
        assert_eq!(p.handle(Action::Ok, OUT, &no), vec![Effect::TvCode]);
        p.on_tv_code(Result::Ok("961234567890".into()));
        assert!(
            matches!(p.stack.last(), Some(Screen::TvCode { code: Scan::Done(c) }) if c[0] == "961234567890")
        );
        assert_eq!(p.handle(Action::Ok, OUT, &no), vec![Effect::TvCode]);
        p.handle(Back, OUT, &no);
        assert!(matches!(p.stack.last(), Some(Screen::Settings { .. })));
        let (mut t, _) = Tube::open(IN);
        t.stack.push(Screen::Settings {
            index: 2,
            groups: false,
        });
        assert_eq!(
            t.handle(Right, IN, &no),
            vec![Effect::Setting(SettingRow::Quality, 1)]
        );
        assert_eq!(
            t.handle(Left, IN, &no),
            vec![Effect::Setting(SettingRow::Quality, -1)]
        );
        t.stack.push(Screen::Settings {
            index: 0,
            groups: false,
        });
        assert_eq!(t.handle(Options, IN, &no), vec![Effect::SignOut(0)]);
    }

    #[test]
    fn settings_are_shown_by_group() {
        let rows = setting_rows(1, true);
        let groups: Vec<SettingGroup> = setting_groups(&rows).iter().map(|(g, _)| *g).collect();
        assert_eq!(
            groups,
            [
                SettingGroup::Accounts,
                SettingGroup::Video,
                SettingGroup::Playback,
                SettingGroup::SponsorBlock,
                SettingGroup::Phone,
                SettingGroup::About
            ]
        );
        // From the top bar's gear: the groups have focus. Down picks the
        // next group, Right goes into it, Down stays inside it, Left on a
        // value changes it, Back returns to the groups, Back again leaves.
        let (mut t, _) = Tube::open(IN);
        t.stack.push(Screen::Settings {
            index: 0,
            groups: true,
        });
        let idx = |t: &Tube| match t.screen() {
            Some(Screen::Settings { index, groups }) => (*index, *groups),
            _ => (usize::MAX, false),
        };
        let rows = setting_rows(IN.accounts, IN.sponsorblock);
        let video = rows.iter().position(|r| *r == SettingRow::Quality).unwrap();
        t.handle(Down, IN, &no);
        assert_eq!(idx(&t), (video, true));
        t.handle(Right, IN, &no);
        assert_eq!(idx(&t), (video, false));
        for _ in 0..5 {
            t.handle(Down, IN, &no);
        }
        assert_eq!(rows[idx(&t).0], SettingRow::Hdr, "stays in Video");
        t.handle(Up, IN, &no);
        assert_eq!(
            t.handle(Left, IN, &no),
            vec![Effect::Setting(SettingRow::Codec, -1)]
        );
        t.handle(Down, IN, &no);
        // A toggle: Left goes back to the groups (OK switches it).
        assert!(t.handle(Left, IN, &no).is_empty());
        assert!(idx(&t).1);
        t.handle(Right, IN, &no);
        t.handle(Back, IN, &no);
        assert!(idx(&t).1);
        t.handle(Back, IN, &no);
        assert!(!matches!(t.screen(), Some(Screen::Settings { .. })));
    }
}
