//! An address inside a sentence or a row of chips.

use super::*;

/// Icon, label and short address. Hover, focus or a click opens the card.
#[component]
pub fn Address(
    pubkey: Pubkey,
    /// Amber for a reason the caller knows, such as a wallet that is not a
    /// member of the multisig it is listed under.
    #[props(default)]
    warn: bool,
) -> Element {
    let described = use_described(pubkey);
    let care = warn || needs_care(&described);
    let collides = !described.collides_with.is_empty();
    let class = if care { "addr warn" } else { "addr" };
    let label = described.full_label();
    let card = use_card();
    let name = label.clone().unwrap_or_else(|| pubkey.to_string());

    rsx! {
        span {
            class: "{class}",
            tabindex: "0",
            role: "button",
            aria_label: "{name}",
            onmounted: move |e| card.mounted(e),
            onmouseenter: move |_| card.enter(),
            onmouseleave: move |_| card.leave(),
            onfocusin: move |_| card.enter(),
            onfocusout: move |_| card.leave(),
            onclick: move |e| {
                e.stop_propagation();
                card.toggle();
            },
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    card.close();
                }
            },
            CategoryIcon { category: described.category }
            if let Some(label) = label {
                span { class: "addr-label", "{label}" }
            }
            code { class: "addr-short", "{short_pubkey(&pubkey)}" }
            BalanceDelta { pubkey }
            if collides {
                span { class: "addr-alert", title: "Another address has this name",
                    Icon { glyph: Glyph::Alert }
                }
            }
            if let Some(at) = card.at() {
                AddressCard { described: described.clone(), at }
            }
        }
    }
}

/// Where the card opens, measured from what it opens over, in viewport
/// pixels. The card is fixed, so no scrolling box clips it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardAt {
    pub left: f64,
    pub top: f64,
    pub bottom: f64,
    /// The height of the window, to open the card upward near its bottom.
    pub viewport: f64,
    /// Set inside a dropdown list: the list's edges, so the card opens
    /// beside the list instead of over the entries under the address.
    pub beside: Option<Beside>,
}

/// A dropdown list's left and right edges, and the room around it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beside {
    pub left: f64,
    pub right: f64,
    /// The width of the window.
    pub viewport: f64,
    /// The root font size, to turn the card's width into pixels.
    pub rem: f64,
}

/// Provided by a dropdown list of addresses: the cards of the rows in it
/// open beside the list. Give the list element `onmounted: beside.mounted`.
#[derive(Clone, Copy)]
pub struct CardBeside(Signal<Option<std::rc::Rc<MountedData>>>);

pub fn use_card_beside() -> CardBeside {
    let list = use_signal(|| None);
    use_context_provider(|| CardBeside(list))
}

impl CardBeside {
    pub fn mounted(mut self, e: Event<MountedData>) {
        self.0.set(Some(e.data()));
    }
}

thread_local! {
    /// The one card on screen. Opening another closes it at once, instead of
    /// letting it wait out its delay beside the new one.
    static OPEN_CARD: std::cell::Cell<Option<Signal<Option<CardAt>>>> =
        const { std::cell::Cell::new(None) };
}

/// Makes `open` the card on screen, closing the one that was.
fn take_slot(open: Signal<Option<CardAt>>) {
    let previous = OPEN_CARD.with(|slot| slot.replace(Some(open)));
    if let Some(mut previous) = previous.filter(|previous| *previous != open) {
        // Gone with its address already, if that left the screen.
        if let Ok(mut card) = previous.try_write() {
            *card = None;
        }
    }
}

/// The open state of a card, shared by the chip and the row.
#[derive(Clone, Copy)]
pub struct CardState {
    open: Signal<Option<CardAt>>,
    mounted: Signal<Option<std::rc::Rc<MountedData>>>,
    /// Bumped on every enter, so a close that was waiting knows it is stale.
    generation: Signal<u64>,
    beside: Option<CardBeside>,
}

pub fn use_card() -> CardState {
    CardState {
        open: use_signal(|| None),
        mounted: use_signal(|| None),
        generation: use_signal(|| 0),
        beside: try_use_context::<CardBeside>(),
    }
}

impl CardState {
    pub fn at(&self) -> Option<CardAt> {
        (self.open)()
    }

    pub fn mounted(mut self, e: Event<MountedData>) {
        self.mounted.set(Some(e.data()));
    }

    pub fn enter(mut self) {
        self.generation += 1;
        if self.open.peek().is_some() {
            return;
        }
        let Some(node) = self.mounted.peek().clone() else {
            return;
        };
        let generation = *self.generation.peek();
        let mut open = self.open;
        let current = self.generation;
        let list = self.beside.and_then(|beside| beside.0.peek().clone());
        spawn(async move {
            let Ok(rect) = node.get_client_rect().await else {
                return;
            };
            let (width, height, rem) = crate::platform::eval(
                "return [window.innerWidth, window.innerHeight, \
                 parseFloat(getComputedStyle(document.documentElement).fontSize)];",
            )
            .join::<(f64, f64, f64)>()
            .await
            .unwrap_or((f64::MAX, f64::MAX, 16.0));
            // A list that has closed measures as nothing: open as usual.
            let measured = match list {
                Some(list) => list.get_client_rect().await.ok(),
                None => None,
            };
            let beside = measured
                .filter(|list| list.size.width > 0.0)
                .map(|list| Beside {
                    left: list.origin.x,
                    right: list.origin.x + list.size.width,
                    viewport: width,
                    rem,
                });
            if *current.peek() == generation {
                take_slot(open);
                open.set(Some(CardAt {
                    left: rect.origin.x,
                    top: rect.origin.y,
                    bottom: rect.origin.y + rect.size.height,
                    viewport: height,
                    beside,
                }));
            }
        });
    }

    /// Close after a moment, so the pointer can cross to the card.
    pub fn leave(mut self) {
        self.generation += 1;
        let generation = *self.generation.peek();
        let mut open = self.open;
        let current = self.generation;
        spawn(async move {
            crate::rt::sleep(std::time::Duration::from_millis(180)).await;
            if *current.peek() == generation {
                open.set(None);
            }
        });
    }

    pub fn toggle(self) {
        if self.open.peek().is_some() {
            self.close();
        } else {
            self.enter();
        }
    }

    pub fn close(mut self) {
        self.generation += 1;
        self.open.set(None);
    }
}
