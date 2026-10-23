//! The head of a detail: what the address is called, what it is, and the
//! whole address.

use super::*;

/// A category badge, the label as a title, a line saying what it is, and the
/// full address with a copy button. `pubkey` is `None` for a wallet with no
/// address yet, such as a hardware wallet.
#[component]
pub fn AddressHeader(
    pubkey: Option<Pubkey>,
    /// The title, where the label is not the right one (a wallet's own
    /// name before the book has it, a stake account).
    #[props(default)]
    title: Option<String>,
    /// The line under the title. Defaults to what the address is.
    #[props(default)]
    subtitle: Option<String>,
    /// The category to show without an address.
    #[props(default)]
    category: Option<Category>,
    /// An icon of its own, for what no category covers.
    #[props(default)]
    glyph: Option<Glyph>,
    /// Beside the title: chips such as "current".
    #[props(default)]
    title_after: Option<Element>,
    /// Under the address: a balance, and the like.
    children: Element,
) -> Element {
    let session = use_context::<Session>();
    let described = pubkey.map(|pubkey| (session.directory)().describe(&pubkey));
    let category = described
        .as_ref()
        .map(|found| found.category)
        .or(category)
        .unwrap_or(Category::Unknown);
    let collides = described
        .as_ref()
        .is_some_and(|found| !found.collides_with.is_empty());
    let title = title
        .or_else(|| described.as_ref().and_then(Described::full_label))
        .or_else(|| pubkey.as_ref().map(short_pubkey))
        .unwrap_or_default();
    let subtitle = subtitle.unwrap_or_else(|| {
        let kind = described.as_ref().and_then(|found| {
            found.facts.iter().find_map(|fact| match fact {
                Fact::WalletKind(kind) => Some(kind.clone()),
                _ => None,
            })
        });
        match kind {
            Some(kind) => format!("{} · {kind}", category.name()),
            None => category.name().to_string(),
        }
    });

    rsx! {
        div { class: "addr-head",
            if let Some(glyph) = glyph {
                span { class: "addr-badge cat cat-contact",
                    Icon { glyph }
                }
            } else {
                span { class: "addr-badge {category_class(category)}",
                    Icon { glyph: category_glyph(category) }
                }
            }
            div { class: "addr-head-body",
                h3 {
                    "{title}"
                    if collides {
                        span { class: "addr-alert", title: "Another address has this name",
                            Icon { glyph: Glyph::Alert }
                        }
                    }
                    if let Some(after) = title_after {
                        {after}
                    }
                }
                if !subtitle.is_empty() {
                    p { class: "muted", "{subtitle}" }
                }
                if let Some(pubkey) = pubkey {
                    div { class: "addr-full",
                        code { class: "mono", "{pubkey}" }
                        CopyButton { text: pubkey.to_string() }
                    }
                }
                {children}
            }
        }
    }
}
