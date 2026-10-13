//! Focus list: a sticky checklist widget. Its title and items are texts edited in place (a click,
//! type, Enter), each item has a box to tick, a last line adds an item, and a button starts a
//! focus countdown that a timer ticks down every second. The property menu clears the ticked
//! items, sets the countdown's length and picks the note's colour. Everything it knows is its state.

use figcraft_plugin_sdk::ui::{Node, menu_action, menu_color, menu_dropdown, render};
use figcraft_plugin_sdk::{Input, Result, Value, WidgetEvent, cancel_timer, entry, json, notify, set_timer};

/// Most items on one list, and the longest title or item.
const MAX_ITEMS: usize = 8;
const MAX_CHARS: u32 = 60;
/// The countdown lengths the menu offers (label, seconds), and the note colours.
const LENGTHS: [(&str, i64); 3] = [("30 seconds", 30), ("1 minute", 60), ("5 minutes", 300)];
const PAPERS: [&str; 4] = ["#FFF3B0", "#D9F2E3", "#DDE9FF", "#FFE0E0"];
const INK: &str = "#2B2B2B";
const WIDTH: f64 = 240.0;

/// What the widget remembers.
#[derive(Clone, Debug, PartialEq)]
struct List {
    title: String,
    /// (text, ticked)
    items: Vec<(String, bool)>,
    /// Seconds a countdown runs, and what's left of the one running (0: none is).
    length: i64,
    left: i64,
    paper: String,
}

impl List {
    /// The list a new widget starts with.
    fn new() -> List {
        let items = ["Write the release notes", "Tag the build", "Tell the team"].map(|t| (t.to_string(), false)).to_vec();
        List { title: "Launch checklist".into(), items, length: 30, left: 0, paper: PAPERS[0].into() }
    }

    fn from(state: &Value) -> List {
        let new = List::new();
        if state.as_object().is_none_or(|o| o.is_empty()) {
            return new;
        }
        let item = |i: &Value| Some((i.get("text")?.as_str()?.to_string(), i.get("done").and_then(Value::as_bool).unwrap_or(false)));
        let length = state["length"].as_i64().filter(|l| LENGTHS.iter().any(|(_, s)| s == l)).unwrap_or(new.length);
        List {
            title: state["title"].as_str().map_or(new.title, str::to_string),
            items: state["items"].as_array().map(|a| a.iter().filter_map(item).take(MAX_ITEMS).collect()).unwrap_or_default(),
            length,
            left: state["left"].as_i64().unwrap_or(0).clamp(0, length),
            paper: state["paper"].as_str().filter(|p| PAPERS.contains(p)).unwrap_or(PAPERS[0]).into(),
        }
    }

    fn state(&self) -> Value {
        let items: Vec<Value> = self.items.iter().map(|(text, done)| json!({"text": text, "done": done})).collect();
        json!({"title": self.title, "items": items, "length": self.length, "left": self.left, "paper": self.paper})
    }

    /// The list after `event`, and what to do about its timer.
    fn after(mut self, event: &WidgetEvent) -> (List, Clock) {
        let mut clock = Clock::Leave;
        let nth = |name: &str, prefix: &str| name.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok());
        match event {
            WidgetEvent::Click(target) if target == "focus" => {
                // The button starts a countdown, or stops the one running.
                (self.left, clock) = if self.left > 0 { (0, Clock::Stop) } else { (self.length, Clock::Tick) };
            }
            WidgetEvent::Click(target) => {
                if let Some((_, done)) = nth(target, "toggle.").and_then(|i| self.items.get_mut(i)) {
                    *done = !*done;
                }
            }
            WidgetEvent::Text { input, text } => {
                let text = text.trim();
                match (input.as_str(), nth(input, "item.")) {
                    ("title", _) if !text.is_empty() => self.title = text.into(),
                    ("new", _) if !text.is_empty() && self.items.len() < MAX_ITEMS => self.items.push((text.into(), false)),
                    // An item left empty goes.
                    (_, Some(i)) if text.is_empty() && i < self.items.len() => drop(self.items.remove(i)),
                    (_, Some(i)) => {
                        if let Some(item) = self.items.get_mut(i) {
                            item.0 = text.into();
                        }
                    }
                    _ => {}
                }
            }
            WidgetEvent::Menu { name, value } => match (name.as_str(), value.as_str()) {
                ("clear", _) => self.items.retain(|(_, done)| !done),
                ("length", Some(label)) => self.length = LENGTHS.iter().find(|(l, _)| *l == label).map_or(self.length, |(_, s)| *s),
                ("paper", Some(p)) if PAPERS.contains(&p) => self.paper = p.into(),
                _ => {}
            },
            WidgetEvent::Timer if self.left > 0 => {
                self.left -= 1;
                clock = if self.left > 0 { Clock::Tick } else { Clock::Done };
            }
            _ => {}
        }
        (self, clock)
    }

    /// "0:42"
    fn clock_text(&self) -> String {
        format!("{}:{:02}", self.left / 60, self.left % 60)
    }

    fn view(&self) -> Node {
        let rule = || Node::line(WIDTH).stroke("#00000022", 1.0).fill_width();
        let items = self.items.iter().enumerate().map(|(i, (text, done))| {
            let mut tick =
                Node::row().name("Tick").target(&format!("toggle.{i}")).size(16.0, 16.0).radius(4.0).justify("center").tooltip("Tick it off");
            tick = if *done {
                tick.fill(INK).child(Node::vector("M2 6 L5 9 L10 3").stroke("#FFFFFF", 2.0))
            } else {
                tick.fill("#FFFFFF").stroke(INK, 1.5).hover(Some("#00000018"), None, None)
            };
            let label = Node::text(text).name("Item").input(&format!("item.{i}"), MAX_CHARS).width(WIDTH - 24.0).font_size(13.0).color(INK);
            Node::row().name("Row").gap(8.0).align("min").child(tick).child(if *done { label.opacity(0.45) } else { label })
        });
        let running = self.left > 0;
        let button = Node::row()
            .name("Focus")
            .target("focus")
            .paddings([6.0, 12.0, 6.0, 12.0])
            .radius(14.0)
            .fill(INK)
            .hover(None, None, Some(0.8))
            .tooltip(if running { "Stop the countdown" } else { "Start a focus countdown" })
            .child(
                Node::text(&if running { format!("Stop · {}", self.clock_text()) } else { "Start focus".into() })
                    .font_size(12.0)
                    .weight(600.0)
                    .color("#FFFFFF"),
            );
        // How much of the countdown is gone.
        let gone = if running { (self.length - self.left) as f64 / self.length.max(1) as f64 } else { 0.0 };
        let track = Node::frame()
            .name("Progress")
            .height(4.0)
            .radius(2.0)
            .fill("#00000018")
            .fill_width()
            .child(Node::rectangle((WIDTH - 110.0) * gone, 4.0).radius(2.0).fill(INK));
        let mut note = Node::column()
            .name("Focus list")
            .width(WIDTH + 32.0)
            .gap(10.0)
            .padding(16.0)
            .radii([2.0, 2.0, 14.0, 2.0])
            .fill(&self.paper)
            .shadow(0.0, 4.0, 12.0, "#00000030")
            .align("min")
            .child(
                Node::text(&self.title)
                    .name("Title")
                    .input("title", MAX_CHARS)
                    .width(WIDTH)
                    .font_size(16.0)
                    .weight(700.0)
                    .color(INK)
                    .tooltip("Click to rename"),
            )
            .child(rule())
            .children(items);
        if self.items.len() < MAX_ITEMS {
            note = note.child(Node::text("Add an item…").name("New item").input("new", MAX_CHARS).width(WIDTH).font_size(13.0).color("#00000066"));
        }
        note.child(rule()).child(Node::row().name("Footer").gap(10.0).width(WIDTH).child(button).child(track))
    }

    fn menu(&self) -> Vec<Value> {
        let labels = LENGTHS.map(|(label, _)| label);
        let length = LENGTHS.iter().find(|(_, s)| *s == self.length).map_or(labels[0], |(l, _)| l);
        vec![
            menu_action("clear", "Clear ticked"),
            menu_dropdown("length", "Countdown", &labels, length),
            menu_color("paper", "Paper", &PAPERS, &self.paper),
        ]
    }
}

/// What an event means for the widget's timer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Clock {
    /// Keep the timer as it is.
    Leave,
    /// Call back in a second.
    Tick,
    Stop,
    /// The countdown just ran out.
    Done,
}

fn run(input: Input) -> Result<Value> {
    let w = input.widget.ok_or("Insert the focus list from Plugins ▸ Widgets")?;
    let (list, clock) = List::from(&w.state).after(&w.event);
    match clock {
        Clock::Tick => set_timer(1000)?,
        Clock::Stop => cancel_timer(),
        Clock::Done => notify(&format!("{}: time's up", list.title)),
        Clock::Leave => {}
    }
    render(&list.state(), list.view(), Some(list.menu()))?;
    Ok(json!({"left": list.left, "items": list.items.len()}))
}

entry!(run);

#[cfg(test)]
mod tests {
    use super::*;

    fn text(input: &str, text: &str) -> WidgetEvent {
        WidgetEvent::Text { input: input.into(), text: text.into() }
    }

    #[test]
    fn items_are_edited_ticked_added_and_removed() {
        let (l, _) = List::from(&json!({})).after(&WidgetEvent::Insert);
        assert_eq!((l.title.as_str(), l.items.len()), ("Launch checklist", 3));
        let (l, _) = l.after(&text("title", "  Ship it  "));
        let (l, _) = l.after(&text("item.1", "Tag v2"));
        let (l, _) = l.after(&text("new", "Celebrate"));
        let (l, _) = l.after(&WidgetEvent::Click("toggle.0".into()));
        assert_eq!(l.title, "Ship it");
        assert_eq!(l.items[1], ("Tag v2".to_string(), false));
        assert_eq!((l.items.len(), l.items[3].0.as_str(), l.items[0].1), (4, "Celebrate", true));
        assert_eq!(List::from(&l.state()), l, "the state round-trips");
        // An emptied item goes; an empty title or new item changes nothing; junk is ignored.
        let (l, _) = l.after(&text("item.2", " "));
        let (l, _) = l.after(&text("title", ""));
        let (l, _) = l.after(&text("new", ""));
        let (l, _) = l.after(&text("item.99", "x"));
        let (l, _) = l.after(&WidgetEvent::Click("toggle.99".into()));
        assert_eq!((l.items.len(), l.title.as_str()), (3, "Ship it"));
        let (l, _) = l.after(&WidgetEvent::Menu { name: "clear".into(), value: Value::Null });
        assert_eq!(l.items.len(), 2);
        let full = (0..20).fold(l, |l, i| l.after(&text("new", &format!("Item {i}"))).0);
        assert_eq!(full.items.len(), MAX_ITEMS);
        assert!(!Value::from(full.view()).to_string().contains("\"new\""), "a full list has no line to add one");
        assert!(run(Input::default()).is_err(), "it only runs as a widget");
    }

    #[test]
    fn the_countdown_ticks_down_and_stops() {
        let l = List::from(&json!({"title": "T", "items": [], "length": 30}));
        let (l, clock) = l.after(&WidgetEvent::Click("focus".into()));
        assert_eq!((l.left, clock, l.clock_text().as_str()), (30, Clock::Tick, "0:30"));
        let (l, clock) = l.after(&WidgetEvent::Timer);
        assert_eq!((l.left, clock), (29, Clock::Tick));
        let (stopped, clock) = l.clone().after(&WidgetEvent::Click("focus".into()));
        assert_eq!((stopped.left, clock), (0, Clock::Stop));
        let (l, clock) = List { left: 1, ..l }.after(&WidgetEvent::Timer);
        assert_eq!((l.left, clock), (0, Clock::Done));
        // A timer that fires with nothing running does nothing.
        assert_eq!(l.clone().after(&WidgetEvent::Timer).1, Clock::Leave);
        let (l, _) = l.after(&WidgetEvent::Menu { name: "length".into(), value: json!("5 minutes") });
        assert_eq!((l.length, l.menu()[1]["value"].as_str()), (300, Some("5 minutes")));
        assert_eq!(List::from(&json!({"title": "T", "length": 7, "left": 99, "paper": "blue"})).length, 30, "junk state falls back");
    }
}
