use std::path::PathBuf;

use gpui::{
    div, prelude::FluentBuilder, App, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, StyledImage,
};
use gpui_component::alert::Alert;
use gpui_component::description_list::DescriptionList;
use gpui_component::group_box::{GroupBox, GroupBoxVariants};
use gpui_component::label::Label;
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::tag::Tag;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};

use crate::listing::{format_mtime, format_size, Entry, EntryKind};

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
        kind: SharedString,
        path: SharedString,
        size: SharedString,
        modified: SharedString,
        hidden: bool,
        note: Option<SharedString>,
    },
    Failed {
        message: SharedString,
    },
}

fn kind_label(kind: &EntryKind) -> SharedString {
    match kind {
        EntryKind::Directory => "Folder".into(),
        EntryKind::File => "File".into(),
        EntryKind::Symlink { .. } => "Link".into(),
    }
}

fn meta_from_entry(entry: &Entry, note: Option<SharedString>) -> Preview {
    Preview::Meta {
        title: entry.name.clone().into(),
        kind: kind_label(&entry.kind),
        path: entry.path.display().to_string().into(),
        size: if entry.is_directory() {
            "—".into()
        } else {
            format_size(entry.size).into()
        },
        modified: format_mtime(entry.modified).into(),
        hidden: entry.hidden,
        note,
    }
}

pub fn build_preview(entry: Entry) -> Preview {
    match &entry.kind {
        EntryKind::Directory => meta_from_entry(&entry, None),
        EntryKind::Symlink { target } => meta_from_entry(
            &entry,
            Some(
                format!(
                    "Symbolic link → {}. Ply does not walk link targets in the Tree.",
                    target.display()
                )
                .into(),
            ),
        ),
        EntryKind::File => {
            let ext = entry
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp") {
                return Preview::Image {
                    path: entry.path.clone(),
                    title: entry.name.clone().into(),
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
                            title: entry.name.clone().into(),
                            body: SharedString::from(text.to_string()),
                            truncated,
                        },
                        Err(_) => meta_from_entry(
                            &entry,
                            Some(
                                format!("{} · not UTF-8 text", format_size(entry.size)).into(),
                            ),
                        ),
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
        Preview::None => frame
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                Icon::new(IconName::Inbox)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(Label::new("Select an entry").secondary("Preview shows metadata, text, or images")),
        Preview::Loading => frame
            .items_center()
            .justify_center()
            .gap_2()
            .child(Spinner::new().small().color(cx.theme().muted_foreground))
            .child(Label::new("Loading preview")),
        Preview::Failed { message } => frame.child(Alert::error("preview-err", message.clone())),
        Preview::Meta {
            title,
            kind,
            path,
            size,
            modified,
            hidden,
            note,
        } => {
            let mut tags = h_flex().gap_1().child(kind_tag(kind));
            if *hidden {
                tags = tags.child(Tag::warning().small().child("Hidden"));
            }
            frame
                .child(Label::new(title.clone()))
                .child(tags)
                .child(
                    GroupBox::new().title("Details").outline().child(
                        DescriptionList::vertical()
                            .columns(1)
                            .bordered(false)
                            .item("Kind", kind.clone(), 1)
                            .item("Size", size.clone(), 1)
                            .item("Modified", modified.clone(), 1)
                            .item("Path", path.clone(), 1),
                    ),
                )
                .when_some(note.clone(), |this, note| this.child(Label::new(note)))
        }
        Preview::Text {
            title,
            body,
            truncated,
        } => frame
            .child(Label::new(title.clone()))
            .when(*truncated, |this| {
                this.child(Tag::warning().small().child("First 1 MB"))
            })
            .child(
                div()
                    .id("preview-text")
                    .flex_1()
                    .overflow_y_scrollbar()
                    .text_sm()
                    .child(body.clone()),
            ),
        Preview::Image { path, title } => frame.child(Label::new(title.clone())).child(
            gpui::img(path.clone())
                .max_w_full()
                .max_h_full()
                .object_fit(gpui::ObjectFit::Contain),
        ),
    }
}

fn kind_tag(kind: &str) -> gpui::AnyElement {
    match kind {
        "Folder" => Tag::info().small().child("Folder").into_any_element(),
        "Link" => Tag::secondary().small().child("Link").into_any_element(),
        _ => Tag::secondary().small().child("File").into_any_element(),
    }
}
