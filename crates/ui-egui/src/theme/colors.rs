//! Semantic UI colors shared by Lizard controls and the standalone catalog.
//! Product surfaces may override a role (for example, the editor's dark panel is #2B2B2B).

use egui::Color32;

macro_rules! define_color_roles {
    ($($role:ident => ($group:literal, $name:literal, $light:expr, $dark:expr, $usage:literal),)*) => {
        /// A semantic color role; use [`Self::value`] for the active mode.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum ColorRole { $($role,)* }

        pub const ALL_COLOR_ROLES: &[ColorRole] = &[$(ColorRole::$role,)*];

        impl ColorRole {
            pub const fn group(self) -> &'static str {
                match self { $(Self::$role => $group,)* }
            }

            pub const fn name(self) -> &'static str {
                match self { $(Self::$role => concat!("--lizard-color-", $name),)* }
            }

            pub const fn usage(self) -> &'static str {
                match self { $(Self::$role => $usage,)* }
            }

            /// Unmultiplied sRGBA, with the alpha byte preserved for display and copying.
            pub const fn packed(self, dark: bool) -> u32 {
                match self { $(Self::$role => if dark { $dark } else { $light },)* }
            }

            pub fn value(self, dark: bool) -> Color32 {
                let rgba = self.packed(dark);
                Color32::from_rgba_unmultiplied((rgba >> 24) as u8, (rgba >> 16) as u8, (rgba >> 8) as u8, rgba as u8)
            }

            pub fn hex(self, dark: bool) -> String {
                let rgba = self.packed(dark);
                if rgba & 0xFF == 0xFF { format!("#{:06X}", rgba >> 8) } else { format!("#{rgba:08X}") }
            }
        }
    };
}

define_color_roles! {
    Bg => ("Surfaces", "bg", 0xFFFFFFFF, 0x2C2C2CFF, "Main surface"),
    BgSecondary => ("Surfaces", "bg-secondary", 0xF5F5F5FF, 0x383838FF, "Inputs and secondary surfaces"),
    BgTertiary => ("Surfaces", "bg-tertiary", 0xE6E6E6FF, 0x444444FF, "Third-level surfaces"),
    BgHover => ("Surfaces", "bg-hover", 0xF5F5F5FF, 0x383838FF, "Hover background"),
    BgPressed => ("Surfaces", "bg-pressed", 0xF5F5F5FF, 0x383838FF, "Pressed background"),
    BgActive => ("Surfaces", "bg-active", 0xF5F5F5FF, 0x383838FF, "Alias of pressed"),
    BgInverse => ("Surfaces", "bg-inverse", 0x2C2C2CFF, 0xFFFFFFFF, "Inverse surface"),
    BgMenu => ("Surfaces", "bg-menu", 0xFFFFFFFF, 0x1E1E1EFF, "Menu surface"),
    BgMenuHover => ("Surfaces", "bg-menu-hover", 0x0C8CE9FF, 0x0C8CE9FF, "Highlighted menu item"),
    BgDisabled => ("Surfaces", "bg-disabled", 0xD9D9D9FF, 0x757575FF, "Disabled fill"),
    BgDisabledSecondary => ("Surfaces", "bg-disabled-secondary", 0xB3B3B3FF, 0xB3B3B3FF, "Alternative disabled fill"),
    BgSwitch => ("Surfaces", "bg-switch", 0xE6E6E6FF, 0x444444FF, "Switch track"),
    BgTransparentHover => ("Surfaces", "bg-transparent-hover", 0x0000000D, 0xFFFFFF0D, "Transparent button hover"),
    BgTransparentPressed => ("Surfaces", "bg-transparent-pressed", 0x0000001A, 0xFFFFFF1A, "Transparent button pressed"),

    BgBrand => ("Brand and status", "bg-brand", 0x0D99FFFF, 0x0C8CE9FF, "Primary action"),
    BgBrandHover => ("Brand and status", "bg-brand-hover", 0x007BE5FF, 0x0A6DC2FF, "Primary action hover"),
    BgBrandPressed => ("Brand and status", "bg-brand-pressed", 0x0768CFFF, 0x105CADFF, "Primary action pressed"),
    BgBrandSecondary => ("Brand and status", "bg-brand-secondary", 0x0768CFFF, 0x105CADFF, "Secondary brand fill"),
    BgBrandTertiary => ("Brand and status", "bg-brand-tertiary", 0xE5F4FFFF, 0x394360FF, "Subtle brand fill"),
    BgComponent => ("Brand and status", "bg-component", 0xF72585FF, 0xF72585FF, "Component fill"),
    BgComponentHover => ("Brand and status", "bg-component-hover", 0xD91F72FF, 0xD91F72FF, "Component hover"),
    BgComponentPressed => ("Brand and status", "bg-component-pressed", 0xBD1B64FF, 0xBD1B64FF, "Component pressed"),
    BgComponentTertiary => ("Brand and status", "bg-component-tertiary", 0xFEE5F0FF, 0x472638FF, "Subtle component fill"),
    BgDanger => ("Brand and status", "bg-danger", 0xF24822FF, 0xE03E1AFF, "Destructive action"),
    BgDangerHover => ("Brand and status", "bg-danger-hover", 0xDC3412FF, 0xC4381CFF, "Destructive hover"),
    BgDangerTertiary => ("Brand and status", "bg-danger-tertiary", 0xFFE2E0FF, 0x60332AFF, "Subtle danger fill"),
    BgSuccess => ("Brand and status", "bg-success", 0x14AE5CFF, 0x198F51FF, "Success state"),
    BgSuccessHover => ("Brand and status", "bg-success-hover", 0x009951FF, 0x078348FF, "Success hover"),
    BgSuccessTertiary => ("Brand and status", "bg-success-tertiary", 0xE0F6E5FF, 0x2F483CFF, "Subtle success fill"),
    BgWarning => ("Brand and status", "bg-warning", 0xFFCD29FF, 0xF3C11BFF, "Warning state"),
    BgWarningHover => ("Brand and status", "bg-warning-hover", 0xFFC21AFF, 0xF2B50DFF, "Warning hover"),
    BgWarningTertiary => ("Brand and status", "bg-warning-tertiary", 0xFFF1C2FF, 0x5C4100FF, "Subtle warning fill"),

    BgSelected => ("Selection", "bg-selected", 0xE5F4FFFF, 0x394360FF, "Selected item"),
    BgSelectedHover => ("Selection", "bg-selected-hover", 0xBDE3FFFF, 0x4A5878FF, "Selected item hover"),
    BgSelectedPressed => ("Selection", "bg-selected-pressed", 0x80CAFFFF, 0x394360FF, "Selected item pressed"),
    BgSelectedSecondary => ("Selection", "bg-selected-secondary", 0xF2F9FFFF, 0x32394DFF, "Subtle selection"),
    BgSelectedStrong => ("Selection", "bg-selected-strong", 0x0D99FFFF, 0x0C8CE9FF, "Strong selection"),
    BgSelectedTertiary => ("Selection", "bg-selected-tertiary", 0xF2F9FFFF, 0x32394DFF, "Discreet selection"),
    BgOnSelected => ("Selection", "bg-onselected", 0xBDE3FFFF, 0x4A5878FF, "Nested selected surface"),

    Text => ("Text", "text", 0x000000E5, 0xFFFFFFFF, "Primary text"),
    TextSecondary => ("Text", "text-secondary", 0x00000099, 0xFFFFFFB2, "Supporting text"),
    TextTertiary => ("Text", "text-tertiary", 0x0000004D, 0xFFFFFF66, "Low-emphasis text"),
    TextDisabled => ("Text", "text-disabled", 0x0000004D, 0xFFFFFF66, "Disabled text"),
    TextHover => ("Text", "text-hover", 0x000000E5, 0xFFFFFFFF, "Hovered text"),
    TextBrand => ("Text", "text-brand", 0x007BE5FF, 0x7CC4F8FF, "Brand link or action"),
    TextComponent => ("Text", "text-component", 0xB9145AFF, 0xFF91BFFF, "Component label"),
    TextDanger => ("Text", "text-danger", 0xDC3412FF, 0xFCA397FF, "Error message"),
    TextSuccess => ("Text", "text-success", 0x009951FF, 0x7CD797FF, "Confirmation"),
    TextWarning => ("Text", "text-warning", 0xB86200FF, 0xF7D15FFF, "Warning copy"),
    TextSelected => ("Text", "text-selected", 0x007BE5FF, 0x7CC4F8FF, "Selected item text"),
    TextOnBrand => ("Text", "text-onbrand", 0xFFFFFFFF, 0xFFFFFFFF, "Text on brand fill"),
    TextOnComponent => ("Text", "text-oncomponent", 0xFFFFFFFF, 0xFFFFFFFF, "Text on component fill"),
    TextOnDanger => ("Text", "text-ondanger", 0xFFFFFFFF, 0xFFFFFFFF, "Text on danger fill"),
    TextOnDisabled => ("Text", "text-ondisabled", 0xFFFFFFFF, 0x2C2C2CFF, "Text on disabled fill"),
    TextOnInverse => ("Text", "text-oninverse", 0xFFFFFFE5, 0x000000E5, "Text on inverse fill"),
    TextOnWarning => ("Text", "text-onwarning", 0x000000E5, 0x000000E5, "Text on warning fill"),
    TextMenu => ("Text", "text-menu", 0xFFFFFFFF, 0xFFFFFFFF, "Text on dark menus"),

    Border => ("Borders", "border", 0xE6E6E6FF, 0x444444FF, "Standard border"),
    BorderStrong => ("Borders", "border-strong", 0x2C2C2CFF, 0xFFFFFFE5, "High-contrast border"),
    BorderSelected => ("Borders", "border-selected", 0x0D99FFFF, 0x0C8CE9FF, "Selected border"),
    BorderSelectedStrong => ("Borders", "border-selected-strong", 0x007BE5FF, 0x7CC4F8FF, "Strong selected border"),
    BorderBrand => ("Borders", "border-brand", 0xBDE3FFFF, 0x105CADFF, "Subtle brand border"),
    BorderBrandStrong => ("Borders", "border-brand-strong", 0x007BE5FF, 0x7CC4F8FF, "Strong brand border"),
    BorderComponent => ("Borders", "border-component", 0xF8B8D5FF, 0x874A64FF, "Subtle component border"),
    BorderComponentHover => ("Borders", "border-component-hover", 0xF72585FF, 0xF72585FF, "Component hover border"),
    BorderComponentStrong => ("Borders", "border-component-strong", 0xC91766FF, 0xFF9BC7FF, "Strong component border"),
    BorderDanger => ("Borders", "border-danger", 0xFFC7C2FF, 0x963323FF, "Invalid field border"),
    BorderDangerStrong => ("Borders", "border-danger-strong", 0xDC3412FF, 0xFCA397FF, "Strong danger border"),
    BorderDisabled => ("Borders", "border-disabled", 0xE6E6E6FF, 0x444444FF, "Disabled border"),
    BorderSuccess => ("Borders", "border-success", 0xB9EBC6FF, 0x517361FF, "Success border"),
    BorderSuccessStrong => ("Borders", "border-success-strong", 0x009951FF, 0x7CD797FF, "Strong success border"),
    BorderWarning => ("Borders", "border-warning", 0xF5DFA8FF, 0x7A5800FF, "Warning border"),
    BorderWarningStrong => ("Borders", "border-warning-strong", 0xB86200FF, 0xF7D15FFF, "Strong warning border"),
    BorderMenu => ("Borders", "border-menu", 0x383838FF, 0x383838FF, "Dark menu border"),
    BorderTranslucent => ("Borders", "border-translucent", 0x0000001A, 0xFFFFFF1A, "Subtle control border"),

    Icon => ("Icons", "icon", 0x000000E5, 0xFFFFFFFF, "Primary icon"),
    IconSecondary => ("Icons", "icon-secondary", 0x00000080, 0xFFFFFFB2, "Supporting icon"),
    IconTertiary => ("Icons", "icon-tertiary", 0x0000004D, 0xFFFFFF66, "Low-emphasis icon"),
    IconBrand => ("Icons", "icon-brand", 0x007BE5FF, 0x7CC4F8FF, "Brand icon"),
    IconComponent => ("Icons", "icon-component", 0xB9145AFF, 0xFF91BFFF, "Component icon"),
    IconDanger => ("Icons", "icon-danger", 0xF24822FF, 0xE03E1AFF, "Destructive icon"),
    IconSuccess => ("Icons", "icon-success", 0x009951FF, 0x7CD797FF, "Success icon"),
    IconWarning => ("Icons", "icon-warning", 0xB86200FF, 0xF7D15FFF, "Warning icon"),
    IconSelected => ("Icons", "icon-selected", 0x007BE5FF, 0x7CC4F8FF, "Selected icon"),
    IconSelectedSecondary => ("Icons", "icon-selected-secondary", 0x0D99FFFF, 0x0C8CE9FF, "Secondary selected icon"),
    IconPressed => ("Icons", "icon-pressed", 0x007BE5FF, 0x0A6DC2FF, "Pressed icon"),
}
