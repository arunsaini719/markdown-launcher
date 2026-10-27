use std::path::Path;

use gpui_kit::{
    App, Div, ElementId, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _,
    component::{
        FocusableExt as _,
        button::{Button, ButtonCustomVariant, ButtonVariants as _},
        tooltip::Tooltip,
    },
    div, rgb,
};

const ACTION_FOREGROUND: u32 = 0xf4f4f5;
const ACTION_BACKGROUNDS: [u32; 3] = [0x27272a, 0x3f3f46, 0x52525b];
const PRIMARY_FOREGROUND: u32 = 0xffffff;
const PRIMARY_BACKGROUNDS: [u32; 3] = [0x2563eb, 0x1d4ed8, 0x1e40af];

// The workspace stays dark even when GPUI's global theme is light for dialogs.
// Specify all button states so hovering never produces white-on-white controls.
pub(crate) fn action_button(id: impl Into<ElementId>, cx: &App) -> Button {
    Button::new(id)
        .focus_ring(false)
        .focus_visible(|style| style.border_color(rgb(0xffffff)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(rgb(ACTION_BACKGROUNDS[0]).into())
                .foreground(rgb(ACTION_FOREGROUND).into())
                .hover(rgb(ACTION_BACKGROUNDS[1]).into())
                .active(rgb(ACTION_BACKGROUNDS[2]).into()),
        )
}

pub(crate) fn primary_button(id: impl Into<ElementId>, cx: &App) -> Button {
    Button::new(id)
        .focus_ring(false)
        .focus_visible(|style| style.border_color(rgb(0xffffff)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(rgb(PRIMARY_BACKGROUNDS[0]).into())
                .foreground(rgb(PRIMARY_FOREGROUND).into())
                .hover(rgb(PRIMARY_BACKGROUNDS[1]).into())
                .active(rgb(PRIMARY_BACKGROUNDS[2]).into()),
        )
}

pub(crate) fn document_header(mode: &str, path: &Path, hint: &str) -> Div {
    let full_path = path.to_string_lossy().into_owned();
    let filename = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .text_color(rgb(0xa1a1aa))
                .child(format!("{mode} - {hint}")),
        )
        .child(
            div()
                .id("document-name")
                .role(gpui_kit::accesskit::Role::Heading)
                .aria_label(filename.clone())
                .aria_description(full_path.clone())
                .debug_selector(|| "document-name".into())
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .text_color(rgb(0xf4f4f5))
                .text_ellipsis()
                .tooltip(move |window, cx| Tooltip::new(full_path.clone()).build(window, cx))
                .child(filename),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(color: u32) -> f64 {
        let channel = |shift: u32| {
            let value = ((color >> shift) & 0xff) as f64 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }

    #[test]
    fn enabled_button_labels_contrast_in_normal_hover_and_active_states() {
        for (foreground, backgrounds) in [
            (ACTION_FOREGROUND, ACTION_BACKGROUNDS),
            (PRIMARY_FOREGROUND, PRIMARY_BACKGROUNDS),
        ] {
            for background in backgrounds {
                let (light, dark) = (luminance(foreground), luminance(background));
                assert!((light + 0.05) / (dark + 0.05) >= 4.5);
            }
        }
    }
}
