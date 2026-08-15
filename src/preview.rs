use std::path::PathBuf;

use gpui::{div, prelude::FluentBuilder, App, IntoElement, ParentElement, SharedString, Styled, StyledImage};
use gpui_component::{alert::Alert, v_flex, ActiveTheme};

use crate::listing::{format_size, Entry, EntryKind};

pub enum Preview {
    None,
    Loading,
    Text {
        title: SharedString,
        body: SharedString,
        truncated: bool,
    },
    Image {
        path: PathBuf,
        title: SharedString,
    },
    Meta {
        title: SharedString,
        body: SharedString,
    },
    Failed {
        message: SharedString,
    },
}

pub fn build_preview(entry: Entry) -> Preview {
    let title = SharedString::from(entry.name.clone());
    match &entry.kind {
        EntryKind::Directory => Preview::Meta {
            title,
            body: SharedString::from(entry.path.display().to_string()),
        },
        EntryKind::Symlink { target } => Preview::Meta {
            title,
            body: SharedString::from(format!(
                "Symbolic link → {}\nPly does not walk link targets in the Tree.",
                target.display()
            )),
        },
        EntryKind::File => {
            let ext = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
                return Preview::Image {
                    path: entry.path,
                    title,
                };
            }
            match std::fs::read(&entry.path) {
                Ok(bytes) => {
                    let truncated = bytes.len() > 1_048_576;
                    let slice = if truncated {
                        &bytes[..1_048_576]
                    } else {
                        &bytes
                    };
                    match std::str::from_utf8(slice) {
                        Ok(text) => Preview::Text {
                            title,
                            body: SharedString::from(text.to_string()),
                            truncated,
                        },
                        Err(_) => Preview::Meta {
                            title,
                            body: SharedString::from(format!(
                                "{} · {} · not UTF-8 text",
                                entry.path.display(),
                                format_size(entry.size)
                            )),
                        },
                    }
                }
                Err(err) => Preview::Failed {
                    message: SharedString::from(err.to_string()),
                },
            }
        }
    }
}

pub fn preview_el(preview: &Preview, cx: &App) -> impl IntoElement + use<> {
    let frame = v_flex()
        .size_full()
        .p_3()
        .gap_2()
        .bg(cx.theme().sidebar);

    match preview {
        Preview::None => frame.child("Select an Entry"),
        Preview::Loading => frame.child("Loading preview…"),
        Preview::Failed { message } => frame.child(Alert::error("preview-err", message.clone())),
        Preview::Meta { title, body } => frame.child(title.clone()).child(body.clone()),
        Preview::Text {
            title,
            body,
            truncated,
        } => frame
            .child(title.clone())
            .when(*truncated, |this| this.child("Showing the first 1 MB."))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_sm()
                    .child(body.clone()),
            ),
        Preview::Image { path, title } => frame.child(title.clone()).child(
            gpui::img(path.clone())
                .max_w_full()
                .max_h_full()
                .object_fit(gpui::ObjectFit::Contain),
        ),
    }
}
