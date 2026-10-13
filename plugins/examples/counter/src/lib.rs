//! Counter: a widget on the canvas with − and + buttons around a count. Each click calls the
//! plugin back with the widget's state; it hands back the new count and the redrawn widget. Its
//! property menu (shown by the selected widget and in the Design panel) resets the count, picks
//! the step of a click and the count's colour; the buttons have a hover style and a tooltip.

use figcraft_plugin_sdk::ui::{Node, menu_action, menu_color, menu_dropdown, menu_link, menu_separator, render};
use figcraft_plugin_sdk::{Input, Result, Value, WidgetEvent, entry, json};

/// The count stays within ± this.
const LIMIT: i64 = 999_999;
/// The steps the menu offers, and the colours for the count.
const STEPS: [&str; 3] = ["1", "5", "10"];
const TINTS: [&str; 4] = ["#1E1E1E", "#0D99FF", "#14AE5C", "#F24822"];

/// What the widget remembers.
#[derive(Clone, Debug, PartialEq)]
struct Counter {
    count: i64,
    step: i64,
    tint: String,
}

impl Counter {
    fn from(state: &Value) -> Counter {
        let tint = state.get("tint").and_then(Value::as_str).filter(|t| TINTS.contains(t)).unwrap_or(TINTS[0]);
        let step = state.get("step").and_then(Value::as_i64).filter(|s| STEPS.contains(&s.to_string().as_str())).unwrap_or(1);
        Counter { count: state.get("count").and_then(Value::as_i64).unwrap_or(0).clamp(-LIMIT, LIMIT), step, tint: tint.into() }
    }

    /// The counter after `event`.
    fn after(mut self, event: &WidgetEvent) -> Counter {
        match event {
            WidgetEvent::Click(target) if target == "increment" => self.count = self.count.saturating_add(self.step),
            WidgetEvent::Click(target) if target == "decrement" => self.count = self.count.saturating_sub(self.step),
            WidgetEvent::Menu { name, .. } if name == "reset" => self.count = 0,
            WidgetEvent::Menu { name, value } if name == "step" => {
                let offered = value.as_str().filter(|s| STEPS.contains(s));
                self.step = offered.and_then(|s| s.parse().ok()).unwrap_or(self.step);
            }
            WidgetEvent::Menu { name, value } if name == "tint" => {
                self.tint = value.as_str().filter(|t| TINTS.contains(t)).unwrap_or(&self.tint).into();
            }
            _ => {}
        }
        self.count = self.count.clamp(-LIMIT, LIMIT);
        self
    }

    fn state(&self) -> Value {
        json!({"count": self.count, "step": self.step, "tint": self.tint})
    }

    /// A square button that calls back with `target`.
    fn button(target: &str, name: &str, label: &str, tip: &str) -> Node {
        Node::row()
            .name(name)
            .target(target)
            .justify("center")
            .size(32.0, 32.0)
            .radius(8.0)
            .fill("#F0F0F0")
            .hover(Some("#DCDCDC"), None, None)
            .tooltip(tip)
            .child(Node::text(label).font_size(16.0).bold().color("#1E1E1E"))
    }

    fn view(&self) -> Node {
        let by = |verb: &str| if self.step == 1 { verb.to_string() } else { format!("{verb} by {}", self.step) };
        Node::row().name("Counter").gap(12.0).padding(12.0).radius(12.0).fill("#FFFFFF").stroke("#D9D9D9", 1.0).children([
            Counter::button("decrement", "Decrease", "−", &by("Decrease")),
            Node::text(&self.count.to_string()).name("Count").font_size(24.0).bold().color(&self.tint),
            Counter::button("increment", "Increase", "+", &by("Increase")),
        ])
    }

    fn menu(&self) -> Vec<Value> {
        vec![
            menu_action("reset", "Reset"),
            menu_dropdown("step", "Step", &STEPS, &self.step.to_string()),
            menu_color("tint", "Colour", &TINTS, &self.tint),
            menu_separator(),
            menu_link("help", "About FigCraft", "https://getartcraft.com/apps/figcraft"),
        ]
    }
}

fn run(input: Input) -> Result<Value> {
    let w = input.widget.ok_or("Insert the counter from Plugins ▸ Widgets")?;
    let counter = Counter::from(&w.state).after(&w.event);
    render(&counter.state(), counter.view(), Some(counter.menu()))?;
    Ok(json!({"count": counter.count}))
}

entry!(run);

#[cfg(test)]
mod tests {
    use super::*;

    fn click(target: &str) -> WidgetEvent {
        WidgetEvent::Click(target.into())
    }

    fn menu(name: &str, value: Value) -> WidgetEvent {
        WidgetEvent::Menu { name: name.into(), value }
    }

    #[test]
    fn counts_within_its_limit() {
        let at = |count: i64| Counter::from(&json!({"count": count}));
        assert_eq!(at(0).after(&WidgetEvent::Insert).count, 0);
        assert_eq!(at(4).after(&click("increment")).count, 5);
        assert_eq!(at(4).after(&click("decrement")).count, 3);
        assert_eq!(at(LIMIT).after(&click("increment")).count, LIMIT);
        assert_eq!(at(i64::MIN).after(&click("decrement")).count, -LIMIT);
        assert_eq!(at(2).after(&click("other")).count, 2);
        assert_eq!(Value::from(at(3).view())["children"][1]["text"], "3");
        assert!(run(Input::default()).is_err(), "it only runs as a widget");
    }

    #[test]
    fn the_menu_resets_steps_and_tints() {
        let c = Counter::from(&json!({"count": 7})).after(&menu("step", json!("5")));
        assert_eq!((c.step, c.clone().after(&click("increment")).count), (5, 12));
        assert_eq!(c.clone().after(&menu("step", json!("7"))).step, 5, "only the offered steps");
        assert_eq!(c.clone().after(&menu("reset", Value::Null)).count, 0);
        let tinted = c.after(&menu("tint", json!("#0D99FF")));
        assert_eq!(tinted.tint, "#0D99FF");
        assert_eq!(tinted.clone().after(&menu("tint", json!("red"))).tint, "#0D99FF");
        assert_eq!(Counter::from(&tinted.state()), tinted, "the state round-trips");
        assert_eq!(tinted.menu()[1]["value"], "5");
        assert_eq!(
            Counter::from(&json!({"step": 3, "tint": "#123456"})),
            Counter { count: 0, step: 1, tint: TINTS[0].into() },
            "junk state falls back"
        );
    }
}
