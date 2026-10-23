use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    Wallet,
    Growth,
    Book,
    Code,
    Layers,
    Pen,
    Sliders,
    Monitor,
    Sun,
    Moon,
    Plus,
    Trash,
    Pencil,
    ChevronLeft,
    ChevronRight,
    Flag,
    Lock,
    Unlock,
    Send,
    Bell,
    Flame,
    Snowflake,
    Device,
    Safe,
    Hourglass,
    Server,
    Coin,
    Pie,
    Person,
    Unknown,
    Copy,
    Check,
    Alert,
    X,
}

impl Glyph {
    fn paths(self) -> &'static [&'static str] {
        match self {
            Glyph::Wallet => &[
                "M19 8V6a1 1 0 0 0-1-1H5.5a2.5 2.5 0 0 0 0 5",
                "M3 7.5v9A2.5 2.5 0 0 0 5.5 19H19a2 2 0 0 0 2-2v-7a2 2 0 0 0-2-2H5.5",
                "M16.5 13.5h.01",
            ],
            Glyph::Growth => &["M3 17l6-6 4 4 8-8", "M15 7h6v6"],
            Glyph::Book => &[
                "M5 4h12a2 2 0 0 1 2 2v14H7a2 2 0 0 1-2-2V4Z",
                "M5 18a2 2 0 0 1 2-2h12",
                "M12 7.25a1.75 1.75 0 1 0 0 3.5 1.75 1.75 0 0 0 0-3.5Z",
                "M9 13.5c.6-1 1.7-1.5 3-1.5s2.4.5 3 1.5",
            ],
            Glyph::Code => &["M8 8l-4 4 4 4", "M16 8l4 4-4 4", "M13.5 5l-3 14"],
            Glyph::Layers => &["M12 3l9 5-9 5-9-5 9-5Z", "M3 12.5l9 5 9-5", "M3 17l9 5 9-5"],
            Glyph::Pen => &[
                "M4 20h4L19.5 8.5a2.1 2.1 0 0 0-3-3L5 17l-1 3Z",
                "M14 7.5l2.5 2.5",
                "M13 20h7",
            ],
            Glyph::Sliders => &[
                "M4 7h9",
                "M17 7h3",
                "M15 5a2 2 0 1 0 0 4 2 2 0 0 0 0-4Z",
                "M4 17h3",
                "M11 17h9",
                "M9 15a2 2 0 1 0 0 4 2 2 0 0 0 0-4Z",
            ],
            Glyph::Monitor => &[
                "M4 5h16a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1Z",
                "M8 20h8",
                "M12 16v4",
            ],
            Glyph::Sun => &[
                "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Z",
                "M12 2v2",
                "M12 20v2",
                "M2 12h2",
                "M20 12h2",
                "M4.9 4.9l1.4 1.4",
                "M17.7 17.7l1.4 1.4",
                "M4.9 19.1l1.4-1.4",
                "M17.7 6.3l1.4-1.4",
            ],
            Glyph::Moon => &["M20 14.5A8 8 0 0 1 9.5 4a7 7 0 1 0 10.5 10.5Z"],
            Glyph::Plus => &["M12 5v14", "M5 12h14"],
            Glyph::Trash => &[
                "M4 7h16",
                "M9 7V5h6v2",
                "M7 7l1 13h8l1-13",
                "M10 11v6M14 11v6",
            ],
            Glyph::Pencil => &["M4 20h4L19 9l-4-4L4 16v4Z", "M13 7l4 4"],
            Glyph::ChevronLeft => &["M15 6l-6 6 6 6"],
            Glyph::ChevronRight => &["M9 6l6 6-6 6"],
            Glyph::Flag => &["M5 21V4", "M5 4h12l-2 4 2 4H5"],
            Glyph::Lock => &[
                "M7 11V8a5 5 0 0 1 10 0v3",
                "M6 11h12a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1Z",
            ],
            Glyph::Send => &[
                "M14.5 21.7a.5.5 0 0 0 .9 0l6.5-19a.5.5 0 0 0-.6-.6l-19 6.5a.5.5 0 0 0 0 .9l7.9 3.2a2 2 0 0 1 1.1 1.1Z",
                "M21.9 2.1 10.9 13.1",
            ],
            Glyph::Unlock => &[
                "M7 11V8a5 5 0 0 1 9.9-1",
                "M6 11h12a1 1 0 0 1 1 1v7a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1Z",
            ],
            Glyph::Bell => &[
                "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9",
                "M10.3 21a1.94 1.94 0 0 0 3.4 0",
            ],
            Glyph::Flame => &[
                "M12 3c1 3 4.5 4.5 4.5 9a4.5 4.5 0 0 1-9 0c0-2 1-3.5 2-4.5.3 1.5 1.2 2.3 2.5 2.5-.3-2.5-.5-4.5 0-7Z",
            ],
            Glyph::Snowflake => &[
                "M12 3v18",
                "M4.2 7.5l15.6 9",
                "M4.2 16.5l15.6-9",
                "M9.5 4.5 12 7l2.5-2.5",
                "M9.5 19.5 12 17l2.5 2.5",
            ],
            Glyph::Device => &[
                "M8 3h8a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H8a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1Z",
                "M10 17h4",
                "M10 7h4v5h-4z",
            ],
            Glyph::Safe => &[
                "M4 4h16a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1Z",
                "M6 19v2",
                "M18 19v2",
                "M12.5 8.5a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z",
                "M12.5 11.5h.01",
                "M6.5 9v5",
            ],
            Glyph::Hourglass => &[
                "M6 3h12",
                "M6 21h12",
                "M7 3c0 5 5 6 5 9s-5 4-5 9",
                "M17 3c0 5-5 6-5 9s5 4 5 9",
                "M9.5 18.5h5",
            ],
            Glyph::Server => &[
                "M4 4h16v6H4z",
                "M4 14h16v6H4z",
                "M7.5 7h.01",
                "M7.5 17h.01",
                "M11 7h5",
                "M11 17h5",
            ],
            Glyph::Pie => &["M21.2 15.9A10 10 0 1 1 8 2.8", "M22 12A10 10 0 0 0 12 2v10Z"],
            Glyph::Coin => &[
                "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z",
                "M8.5 12h7",
                "M12 8.5v7",
            ],
            Glyph::Person => &[
                "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8Z",
                "M4 21c1-4 4-6 8-6s7 2 8 6",
            ],
            Glyph::Unknown => &[
                "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Z",
                "M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.3-1 .8-1 1.5",
                "M12 16.5h.01",
            ],
            Glyph::Copy => &[
                "M9 9h10a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V10a1 1 0 0 1 1-1Z",
                "M16 9V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3",
            ],
            Glyph::Check => &["M5 12.5l4.5 4.5L19 7.5"],
            Glyph::Alert => &["M12 4 2.5 20h19L12 4Z", "M12 10v4", "M12 17h.01"],
            Glyph::X => &["M6 6l12 12", "M18 6 6 18"],
        }
    }
}

/// A stroke icon on a 24px grid, drawn in the current text color.
#[component]
pub fn Icon(glyph: Glyph) -> Element {
    rsx! {
        svg {
            class: "icon",
            view_box: "0 0 24 24",
            "aria-hidden": "true",
            for d in glyph.paths() {
                path { d: "{d}" }
            }
        }
    }
}
