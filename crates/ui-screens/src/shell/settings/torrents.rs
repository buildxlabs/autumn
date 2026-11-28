//! Settings → Torrents: the "Auto-select profiles" editor (#228).
//!
//! Profile CRUD lives in `settings::AutoSelectSettings`; this module only
//! renders `gpui-component` setting rows and calls it. Saves are live: every
//! change revalidates and writes the `auto_select` row.

use gpui_kit::{
    App, AppContext as _, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window,
    component::{
        Disableable, Selectable, Sizable,
        button::{Button, ButtonGroup, ButtonVariants},
        input::{Input, InputEvent, InputState},
        label::Label,
        radio::Radio,
        setting::{SettingField, SettingItem},
    },
    div,
    prelude::FluentBuilder as _,
};
use settings::{
    AutoSelectProfile, AutoSelectSettings, Codec, DEFAULT_PROFILE_NAME, Preference, Source,
    move_item,
};

use super::super::AppShell;

/// Which ordered list a chip belongs to (#181 Q9 ordered ranking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipField {
    Resolutions,
    ReleaseGroups,
    Codecs,
    PreferredLanguages,
    Sources,
}

/// A chip being dragged to reorder an ordered list.
#[derive(Clone)]
struct ChipDrag {
    profile: String,
    field: ChipField,
    from: usize,
    label: String,
}

impl Render for ChipDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().px_2().py_1().child(self.label.clone())
    }
}

/// Which `Preference` axis a segmented control edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefAxis {
    MultipleAudio,
    Batch,
    MultipleSubs,
}

const CODECS: [(Codec, &str); 6] = [
    (Codec::H264, "H.264"),
    (Codec::H265, "H.265"),
    (Codec::Av1, "AV1"),
    (Codec::Xvid, "Xvid"),
    (Codec::Vp9, "VP9"),
    (Codec::Mpeg2, "MPEG-2"),
];

const SOURCES: [(Source, &str); 7] = [
    (Source::BluRay, "Blu-ray"),
    (Source::BdRip, "BDRip"),
    (Source::WebDl, "WEB-DL"),
    (Source::WebRip, "WEBRip"),
    (Source::Dvd, "DVD"),
    (Source::DvdRip, "DVDRip"),
    (Source::Tv, "TV"),
];

fn codec_label(codec: Codec) -> &'static str {
    CODECS
        .iter()
        .find(|(c, _)| *c == codec)
        .map_or("codec", |(_, label)| label)
}

fn source_label(source: Source) -> &'static str {
    SOURCES
        .iter()
        .find(|(s, _)| *s == source)
        .map_or("source", |(_, label)| label)
}

impl AppShell {
    // --- profile state ---

    fn selected_profile(&self) -> AutoSelectProfile {
        self.auto_select
            .profiles
            .iter()
            .find(|profile| profile.name == self.selected_profile)
            .cloned()
            .unwrap_or_default()
    }

    fn edit_selected_profile(
        &mut self,
        change: impl FnOnce(&mut AutoSelectProfile),
        cx: &mut Context<'_, Self>,
    ) {
        let name = self.selected_profile.clone();
        self.update_auto_select(
            move |settings| {
                if let Some(profile) = settings.profiles.iter_mut().find(|p| p.name == name) {
                    change(profile);
                }
            },
            cx,
        );
    }

    /// Applies one auto-select change, validates it and saves it without waiting.
    fn update_auto_select(
        &mut self,
        change: impl FnOnce(&mut AutoSelectSettings),
        cx: &mut Context<'_, Self>,
    ) {
        change(&mut self.auto_select);
        self.commit_auto_select(cx);
        cx.notify();
    }

    /// Persists the current auto-select profiles. A newer save cancels the older one.
    fn commit_auto_select(&mut self, cx: &mut Context<'_, Self>) {
        if let Err(error) = self.auto_select.validate() {
            self.auto_select_error = Some(error.to_string());
            return;
        }
        self.auto_select_error = None;
        let task = (self.auto_select_services.save)(&self.auto_select, cx);
        self.save_auto_select_task = Some(cx.spawn(async move |_, _| {
            task.await;
        }));
    }

    // --- profile CRUD (delegates to `settings`, never inside `render()`) ---

    fn select_profile(&mut self, name: String, cx: &mut Context<'_, Self>) {
        self.selected_profile = name;
        self.renaming_profile = false;
        cx.notify();
    }

    fn add_profile(&mut self, cx: &mut Context<'_, Self>) {
        let mut created = None;
        self.update_auto_select(
            |settings| {
                let base = "New profile";
                let mut name = base.to_owned();
                let mut counter = 2;
                while settings
                    .profiles
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&name))
                {
                    name = format!("{base} {counter}");
                    counter += 1;
                }
                if settings.add_profile(&name).is_ok() {
                    created = Some(name);
                }
            },
            cx,
        );
        if let Some(name) = created {
            self.selected_profile = name;
            cx.notify();
        }
    }

    fn duplicate_profile(&mut self, cx: &mut Context<'_, Self>) {
        let name = self.selected_profile.clone();
        let mut created = None;
        self.update_auto_select(
            |settings| {
                if let Ok(copy) = settings.duplicate_profile(&name) {
                    created = Some(copy);
                }
            },
            cx,
        );
        if let Some(name) = created {
            self.selected_profile = name;
            cx.notify();
        }
    }

    fn delete_profile(&mut self, cx: &mut Context<'_, Self>) {
        let name = self.selected_profile.clone();
        self.update_auto_select(
            move |settings| {
                let _ = settings.delete_profile(&name);
            },
            cx,
        );
        if !self
            .auto_select
            .profiles
            .iter()
            .any(|p| p.name == self.selected_profile)
        {
            DEFAULT_PROFILE_NAME.clone_into(&mut self.selected_profile);
        }
        self.renaming_profile = false;
        cx.notify();
    }

    fn reset_default_profile(&mut self, cx: &mut Context<'_, Self>) {
        self.update_auto_select(AutoSelectSettings::reset_default_profile, cx);
    }

    fn set_global_default(&mut self, name: String, cx: &mut Context<'_, Self>) {
        self.update_auto_select(
            move |settings| {
                let _ = settings.set_global_default(&name);
            },
            cx,
        );
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let current = self.selected_profile.clone();
        self.rename_input
            .update(cx, |input, cx| input.set_value(current, window, cx));
        self.renaming_profile = true;
        cx.notify();
    }

    fn commit_rename(&mut self, cx: &mut Context<'_, Self>) {
        let old = self.selected_profile.clone();
        let new_name = self.rename_input.read(cx).value().trim().to_owned();
        let mut renamed = false;
        self.update_auto_select(
            |settings| {
                if settings.rename_profile(&old, &new_name).is_ok() {
                    renamed = true;
                }
            },
            cx,
        );
        if renamed {
            self.selected_profile = new_name;
            self.renaming_profile = false;
            cx.notify();
        }
    }

    // --- ordered chips ---

    fn move_chip(&mut self, field: ChipField, from: usize, to: usize, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| match field {
                ChipField::Resolutions => move_item(&mut profile.resolutions, from, to),
                ChipField::ReleaseGroups => move_item(&mut profile.release_groups, from, to),
                ChipField::Codecs => move_item(&mut profile.codecs, from, to),
                ChipField::PreferredLanguages => {
                    move_item(&mut profile.preferred_languages, from, to);
                }
                ChipField::Sources => move_item(&mut profile.sources, from, to),
            },
            cx,
        );
    }

    fn remove_chip(&mut self, field: ChipField, index: usize, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| match field {
                ChipField::Resolutions => {
                    if index < profile.resolutions.len() {
                        profile.resolutions.remove(index);
                    }
                }
                ChipField::ReleaseGroups => {
                    if index < profile.release_groups.len() {
                        profile.release_groups.remove(index);
                    }
                }
                ChipField::Codecs => {
                    if index < profile.codecs.len() {
                        profile.codecs.remove(index);
                    }
                }
                ChipField::PreferredLanguages => {
                    if index < profile.preferred_languages.len() {
                        profile.preferred_languages.remove(index);
                    }
                }
                ChipField::Sources => {
                    if index < profile.sources.len() {
                        profile.sources.remove(index);
                    }
                }
            },
            cx,
        );
    }

    fn add_resolution(&mut self, value: u32, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| {
                if !profile.resolutions.contains(&value) {
                    profile.resolutions.push(value);
                }
            },
            cx,
        );
    }

    fn add_release_group(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return;
        }
        self.edit_selected_profile(
            move |profile| {
                if !profile
                    .release_groups
                    .iter()
                    .any(|g| g.eq_ignore_ascii_case(&value))
                {
                    profile.release_groups.push(value);
                }
            },
            cx,
        );
    }

    fn add_preferred_language(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return;
        }
        self.edit_selected_profile(
            move |profile| {
                if !profile
                    .preferred_languages
                    .iter()
                    .any(|l| l.eq_ignore_ascii_case(&value))
                {
                    profile.preferred_languages.push(value);
                }
            },
            cx,
        );
    }

    fn add_codec(&mut self, value: Codec, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| {
                if !profile.codecs.contains(&value) {
                    profile.codecs.push(value);
                }
            },
            cx,
        );
    }

    fn add_source(&mut self, value: Source, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| {
                if !profile.sources.contains(&value) {
                    profile.sources.push(value);
                }
            },
            cx,
        );
    }

    fn add_excluded_term(&mut self, value: &str, cx: &mut Context<'_, Self>) {
        let value = value.trim().to_owned();
        if value.is_empty() {
            return;
        }
        self.edit_selected_profile(
            move |profile| {
                if !profile
                    .excluded_terms
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(&value))
                {
                    profile.excluded_terms.push(value);
                }
            },
            cx,
        );
    }

    fn remove_excluded_term(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| {
                if index < profile.excluded_terms.len() {
                    profile.excluded_terms.remove(index);
                }
            },
            cx,
        );
    }

    fn set_preference(&mut self, axis: PrefAxis, value: Preference, cx: &mut Context<'_, Self>) {
        self.edit_selected_profile(
            move |profile| match axis {
                PrefAxis::MultipleAudio => profile.multiple_audio = value,
                PrefAxis::Batch => profile.batch = value,
                PrefAxis::MultipleSubs => profile.multiple_subs = value,
            },
            cx,
        );
    }
}

/// The Settings → Torrents page: the "Auto-select profiles" section.
#[expect(
    clippy::too_many_lines,
    reason = "the Torrents tab renders every profile field as a setting row"
)]
pub(super) fn render(
    shell: &Entity<AppShell>,
    state: &AppShell,
) -> (&'static str, Vec<SettingItem>) {
    use gpui_kit::component::setting::NumberFieldOptions;

    let profile = state.selected_profile();
    let profile_name = profile.name.clone();
    let is_default = profile_name == DEFAULT_PROFILE_NAME;
    let profiles = state.auto_select.profiles.clone();
    let global_default = state.auto_select.default_profile.clone();
    let selected = state.selected_profile.clone();
    let renaming = state.renaming_profile;
    let rename_input = state.rename_input.clone();
    let error = state.auto_select_error.clone();

    let mut items = vec![profile_list_item(shell, profiles, global_default, selected)];
    items.push(action_bar_item(shell, profile_name.clone(), is_default));

    if renaming && !is_default {
        items.push(rename_row_item(shell, rename_input));
    }

    // --- every AutoSelectProfile field ---

    // ordered `resolutions`
    items.push(ordered_list_item(
        shell,
        "resolutions",
        "Resolutions",
        "Vertical resolutions in lines, best first. Unknown resolutions rank last.",
        ChipField::Resolutions,
        &profile_name,
        profile
            .resolutions
            .iter()
            .map(|r| format!("{r}p"))
            .collect(),
        number_chip_adder(
            shell.clone(),
            "resolutions",
            "e.g. 1080",
            NumberAdd::Resolution,
        ),
    ));

    // ordered `release_groups`
    items.push(ordered_list_item(
        shell,
        "release-groups",
        "Release groups",
        "Fansub groups, best first. Matched case-insensitively.",
        ChipField::ReleaseGroups,
        &profile_name,
        profile.release_groups.clone(),
        text_chip_adder(
            shell.clone(),
            "release-groups",
            "Group name",
            ChipAdd::ReleaseGroup,
        ),
    ));

    // ordered `codecs`
    items.push(ordered_list_item(
        shell,
        "codecs",
        "Codecs",
        "Video codecs, best first. Empty = no preference.",
        ChipField::Codecs,
        &profile_name,
        profile
            .codecs
            .iter()
            .map(|c| codec_label(*c).to_owned())
            .collect(),
        codec_adder(shell.clone()),
    ));

    // ordered `preferred_languages`
    items.push(ordered_list_item(
        shell,
        "preferred-languages",
        "Preferred languages",
        "Subtitle or audio language tags (ENG, VOSTFR), best first, matched as words.",
        ChipField::PreferredLanguages,
        &profile_name,
        profile.preferred_languages.clone(),
        text_chip_adder(
            shell.clone(),
            "preferred-languages",
            "e.g. ENG",
            ChipAdd::PreferredLanguage,
        ),
    ));

    // ordered `sources`
    items.push(ordered_list_item(
        shell,
        "sources",
        "Sources",
        "Video sources, best first. Empty = no preference.",
        ChipField::Sources,
        &profile_name,
        profile
            .sources
            .iter()
            .map(|s| source_label(*s).to_owned())
            .collect(),
        source_adder(shell.clone()),
    ));

    // `min_seeders`
    {
        let set_shell = shell.clone();
        items.push(
            SettingItem::new(
                "Minimum seeders",
                SettingField::number_input(
                    NumberFieldOptions {
                        min: 0.0,
                        max: f64::from(u32::MAX),
                        step: 1.0,
                    },
                    {
                        let val_shell = shell.clone();
                        move |cx: &App| f64::from(val_shell.read(cx).selected_profile().min_seeders)
                    },
                    move |value, cx| {
                        let value = value
                            .clamp(0.0, f64::from(u32::MAX))
                            .round()
                            .to_string()
                            .parse::<u32>()
                            .unwrap_or(0);
                        set_shell.update(cx, |shell, cx| {
                            shell.edit_selected_profile(
                                move |profile| profile.min_seeders = value,
                                cx,
                            );
                        });
                    },
                ),
            )
            .description("Releases with fewer seeders are dropped; unknown counts as zero."),
        );
    }

    // `min_size`
    items.push(optional_size_item(
        shell,
        "min-size",
        "Minimum size",
        "Releases smaller than this many bytes, or of unknown size, are dropped.",
        profile.min_size,
        SizeBound::Min,
    ));
    // `max_size`
    items.push(optional_size_item(
        shell,
        "max-size",
        "Maximum size",
        "Releases larger than this many bytes, or of unknown size, are dropped.",
        profile.max_size,
        SizeBound::Max,
    ));

    // `excluded_terms` tag input
    items.push(excluded_terms_item(shell, profile.excluded_terms.clone()));

    // preference axes: Prefer / Neutral / Avoid (+ Only / Never where #181 allows)
    items.push(preference_item(
        shell,
        "Dual audio",
        PrefAxis::MultipleAudio,
        profile.multiple_audio,
        true,
        "Releases with multiple audio tracks.",
    ));
    items.push(preference_item(
        shell,
        "Batch",
        PrefAxis::Batch,
        profile.batch,
        true,
        "A batch containing several episodes vs a single episode.",
    ));
    items.push(preference_item(
        shell,
        "Multiple subtitles",
        PrefAxis::MultipleSubs,
        profile.multiple_subs,
        true,
        "Releases with multiple subtitle languages.",
    ));

    // require-known hard filters
    items.push(switch_item(
        shell,
        "Require known codec",
        "Drop releases whose codec is not in the codec list.",
        profile.require_codec,
        move |value, shell, cx| {
            shell.edit_selected_profile(move |p| p.require_codec = value, cx);
        },
    ));
    items.push(switch_item(
        shell,
        "Require known source",
        "Drop releases whose source is not in the source list.",
        profile.require_source,
        move |value, shell, cx| {
            shell.edit_selected_profile(move |p| p.require_source = value, cx);
        },
    ));
    items.push(switch_item(
        shell,
        "Require known language",
        "Drop releases naming none of the preferred languages.",
        profile.require_language,
        move |value, shell, cx| {
            shell.edit_selected_profile(move |p| p.require_language = value, cx);
        },
    ));

    // validation error
    items.push(SettingItem::render(move |_, _, _| {
        error.clone().map_or_else(
            || div().into_any_element(),
            |message| Label::new(message).text_sm().into_any_element(),
        )
    }));

    ("Torrents", items)
}

fn profile_list_item(
    shell: &Entity<AppShell>,
    profiles: Vec<AutoSelectProfile>,
    global_default: String,
    selected: String,
) -> SettingItem {
    let shell = shell.clone();
    SettingItem::render(move |_, _, _| {
        let shell = shell.clone();
        let mut column = div().flex().flex_col().gap_2();
        column = column.child(Label::new("Auto-select profiles").text_lg());
        column = column.child(
            Label::new("Ordered filters and ranking used to pick a torrent. The radio marks the global default; a per-media override wins for that media.")
                .text_sm(),
        );
        let mut list = div().flex().flex_col().gap_1();
        for profile in &profiles {
            let name = profile.name.clone();
            let radio_shell = shell.clone();
            let radio_name = name.clone();
            let pick_shell = shell.clone();
            let pick_name = name.clone();
            let checked_default = name == global_default;
            let is_selected = name == selected;
            list = list.child(
                div()
                    .id(SharedString::from(format!("profile-row-{name}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Radio::new(SharedString::from(format!("profile-default-{name}")))
                            .checked(checked_default)
                            .on_click(move |_, _, cx| {
                                radio_shell.update(cx, |shell, cx| {
                                    shell.set_global_default(radio_name.clone(), cx);
                                });
                            }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("profile-pick-{name}")))
                            .label(name)
                            .selected(is_selected)
                            .on_click(move |_, _, cx| {
                                pick_shell.update(cx, |shell, cx| {
                                    shell.select_profile(pick_name.clone(), cx);
                                });
                            }),
                    )
                    .when(checked_default, |row| {
                        row.child(Label::new("global default").text_sm())
                    }),
            );
        }
        list = list.child(
            Button::new("profile-new")
                .label("New")
                .on_click(move |_, _, cx| {
                    shell.update(cx, AppShell::add_profile);
                }),
        );
        column.child(list).into_any_element()
    })
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "whether the shipped Default is selected"
)]
fn action_bar_item(
    shell: &Entity<AppShell>,
    profile_name: String,
    is_default: bool,
) -> SettingItem {
    let shell = shell.clone();
    SettingItem::render(move |_, _, _| {
        let dup_shell = shell.clone();
        let del_shell = shell.clone();
        let rename_shell = shell.clone();
        let reset_shell = shell.clone();
        let mut row = div().flex().items_center().gap_2();
        row = row.child(Label::new(format!("Editing: {profile_name}")).text_sm());
        row = row.child(
            Button::new("profile-rename")
                .label("Rename")
                .disabled(is_default)
                .on_click(move |_, window, cx| {
                    rename_shell.update(cx, |shell, cx| shell.begin_rename(window, cx));
                }),
        );
        row = row.child(
            Button::new("profile-duplicate")
                .label("Duplicate")
                .on_click(move |_, _, cx| {
                    dup_shell.update(cx, AppShell::duplicate_profile);
                }),
        );
        if is_default {
            row = row.child(Button::new("profile-reset").label("Reset").on_click(
                move |_, _, cx| {
                    reset_shell.update(cx, AppShell::reset_default_profile);
                },
            ));
        }
        row = row.child(
            Button::new("profile-delete")
                .label("Delete")
                .disabled(is_default)
                .on_click(move |_, _, cx| {
                    del_shell.update(cx, AppShell::delete_profile);
                }),
        );
        row.into_any_element()
    })
}

fn rename_row_item(shell: &Entity<AppShell>, rename_input: Entity<InputState>) -> SettingItem {
    let shell = shell.clone();
    SettingItem::render(move |_, _, _| {
        let save_shell = shell.clone();
        let cancel_shell = shell.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(Input::new(&rename_input))
            .child(
                Button::new("profile-rename-save")
                    .label("Save")
                    .on_click(move |_, _, cx| {
                        save_shell.update(cx, AppShell::commit_rename);
                    }),
            )
            .child(
                Button::new("profile-rename-cancel")
                    .label("Cancel")
                    .on_click(move |_, _, cx| {
                        cancel_shell.update(cx, |shell, cx| {
                            shell.renaming_profile = false;
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    })
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "whether the Only/Never segments are offered"
)]
fn preference_item(
    shell: &Entity<AppShell>,
    title: &str,
    axis: PrefAxis,
    current: Preference,
    allow_only_never: bool,
    description: &str,
) -> SettingItem {
    let shell = shell.clone();
    let title = title.to_owned();
    let description = description.to_owned();
    let id_prefix = format!("pref-{axis:?}");
    SettingItem::render(move |_, _, _| {
        let shell = shell.clone();
        let table = preference_table(allow_only_never);
        let mut labels = vec![
            SharedString::from("Prefer"),
            SharedString::from("Neutral"),
            SharedString::from("Avoid"),
        ];
        if allow_only_never {
            labels.push(SharedString::from("Only"));
            labels.push(SharedString::from("Never"));
        }
        let mut group = ButtonGroup::new(SharedString::from(id_prefix.clone()))
            .compact()
            .outline();
        let selected_index = table.iter().position(|p| *p == current);
        for (index, label) in labels.into_iter().enumerate() {
            group = group.child(
                Button::new(SharedString::from(format!("{id_prefix}-{label}")))
                    .label(label)
                    .selected(Some(index) == selected_index),
            );
        }
        let table_for_click = table;
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Label::new(title.clone()))
            .child(Label::new(description.clone()).text_sm())
            .child(group.on_click(move |clicks: &Vec<usize>, _, cx| {
                if let Some(&index) = clicks.first()
                    && let Some(&value) = table_for_click.get(index)
                {
                    shell.update(cx, |shell, cx| shell.set_preference(axis, value, cx));
                }
            }))
            .into_any_element()
    })
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "whether to include Only/Never"
)]
fn preference_table(allow_only_never: bool) -> Vec<Preference> {
    let mut table = vec![Preference::Prefer, Preference::Neutral, Preference::Avoid];
    if allow_only_never {
        table.push(Preference::Only);
        table.push(Preference::Never);
    }
    table
}

#[derive(Clone, Copy)]
enum SizeBound {
    Min,
    Max,
}

fn optional_size_item(
    shell: &Entity<AppShell>,
    id: &'static str,
    title: &str,
    description: &str,
    current: Option<u64>,
    bound: SizeBound,
) -> SettingItem {
    let shell = shell.clone();
    let title = title.to_owned();
    let description = description.to_owned();
    SettingItem::render(move |_, window, cx| {
        let shown = current.map_or_else(String::new, |v| v.to_string());
        let key = format!("size-{id}");
        let size_shell = shell.clone();
        let state = window.use_keyed_state(SharedString::from(key), cx, move |window, cx| {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(shown.clone())
                    .placeholder("unset")
            });
            let size_shell = size_shell.clone();
            let subscription = cx.subscribe(&input, move |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let text = input.read(cx).value().trim().to_owned();
                    let parsed = if text.is_empty() {
                        None
                    } else {
                        text.parse::<u64>().ok()
                    };
                    if text.is_empty() || parsed.is_some() {
                        size_shell.update(cx, |shell, cx| {
                            shell.edit_selected_profile(
                                move |profile| match bound {
                                    SizeBound::Min => profile.min_size = parsed,
                                    SizeBound::Max => profile.max_size = parsed,
                                },
                                cx,
                            );
                        });
                    }
                }
            });
            SizeState {
                input,
                _subscription: subscription,
            }
        });
        // sync displayed value when the underlying value changed externally
        let shown = current.map_or_else(String::new, |v| v.to_string());
        state.update(cx, |state, cx| {
            if state.input.read(cx).value() != shown {
                state
                    .input
                    .update(cx, |input, cx| input.set_value(shown.clone(), window, cx));
            }
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Label::new(title.clone()))
            .child(Label::new(description.clone()).text_sm())
            .child(Input::new(&state.read(cx).input))
            .into_any_element()
    })
}

struct SizeState {
    input: Entity<InputState>,
    _subscription: gpui_kit::Subscription,
}

fn excluded_terms_item(shell: &Entity<AppShell>, terms: Vec<String>) -> SettingItem {
    let shell = shell.clone();
    SettingItem::render(move |_, window, cx| {
        let add = keyed_input(
            window,
            cx,
            "chips-add-excluded-terms".to_owned(),
            "e.g. CAM",
        );
        let add_shell = shell.clone();
        let add_input = add.clone();
        let mut row = div().flex().flex_wrap().gap_1().items_center();
        for (index, term) in terms.iter().enumerate() {
            let remove_shell = shell.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("excluded-term-{index}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .child(Label::new(term.clone()).text_sm())
                    .child(
                        Button::new(format!("excluded-term-{index}-remove"))
                            .label("×")
                            .small()
                            .ghost()
                            .on_click(move |_, _, cx| {
                                remove_shell
                                    .update(cx, |shell, cx| shell.remove_excluded_term(index, cx));
                            }),
                    ),
            );
        }
        let mut column = div().flex().flex_col().gap_1();
        column = column.child(Label::new("Excluded terms"));
        column = column.child(
            Label::new(
                "Releases whose name contains any of these, case-insensitively, are dropped.",
            )
            .text_sm(),
        );
        column = column.child(row);
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Input::new(&add))
                .child(Button::new("excluded-terms-add").label("Add").on_click(
                    move |_, window, cx| {
                        let text = add_input.read(cx).value().trim().to_owned();
                        if !text.is_empty() {
                            add_shell.update(cx, |shell, cx| shell.add_excluded_term(&text, cx));
                            add_input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    },
                )),
        );
        column.into_any_element()
    })
}

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "a switch has one boolean state"
)]
fn switch_item(
    shell: &Entity<AppShell>,
    title: &str,
    description: &str,
    checked: bool,
    on_change: impl Fn(bool, &mut AppShell, &mut Context<'_, AppShell>) + 'static,
) -> SettingItem {
    let shell = shell.clone();
    let title = title.to_owned();
    let description = description.to_owned();
    let on_change = std::rc::Rc::new(on_change);
    SettingItem::render(move |_, _, _| {
        let shell = shell.clone();
        let on_change = std::rc::Rc::clone(&on_change);
        let id = SharedString::from(title.clone());
        gpui_kit::component::switch::Switch::new(id)
            .label(title.clone())
            .checked(checked)
            .on_click(move |checked, _, cx| {
                let on_change = std::rc::Rc::clone(&on_change);
                let value = *checked;
                shell.update(cx, |shell, cx| on_change(value, shell, cx));
            })
            .into_any_element()
    })
    .description(description)
}

/// One profile ordered-list field: reorderable chips + add + remove.
#[expect(clippy::too_many_arguments, reason = "one ordered-list setting row")]
fn ordered_list_item(
    shell: &Entity<AppShell>,
    id: &str,
    title: &str,
    description: &str,
    field: ChipField,
    profile_name: &str,
    labels: Vec<String>,
    add: impl Fn(&mut Window, &mut App, &Entity<AppShell>) -> gpui_kit::AnyElement + 'static,
) -> SettingItem {
    let shell = shell.clone();
    let profile_name = profile_name.to_owned();
    let id = id.to_owned();
    let title = title.to_owned();
    let description = description.to_owned();
    SettingItem::render(move |_, window, cx| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(Label::new(title.clone()))
            .child(Label::new(description.clone()).text_sm())
            .child(ordered_chips(
                &shell,
                &id,
                field,
                &profile_name,
                labels.clone(),
            ))
            .child(add(window, cx, &shell))
            .into_any_element()
    })
}

fn ordered_chips(
    shell: &Entity<AppShell>,
    id: &str,
    field: ChipField,
    profile_name: &str,
    labels: Vec<String>,
) -> impl IntoElement {
    let mut row = div().flex().flex_wrap().gap_1().items_center();
    for (index, label) in labels.into_iter().enumerate() {
        let chip_id = format!("{id}-{index}");
        let shell_drop = shell.clone();
        let shell_remove = shell.clone();
        let profile = profile_name.to_owned();
        row = row.child(
            div()
                .id(SharedString::from(chip_id.clone()))
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .child(Label::new(label.clone()).text_sm())
                .on_drag(
                    ChipDrag {
                        profile: profile.clone(),
                        field,
                        from: index,
                        label,
                    },
                    |drag, _, _, cx| cx.new(|_| drag.clone()),
                )
                .on_drop(move |drag: &ChipDrag, _, cx| {
                    if drag.field == field && drag.profile == profile {
                        let from = drag.from;
                        shell_drop.update(cx, |shell, cx| shell.move_chip(field, from, index, cx));
                    }
                })
                .child(
                    Button::new(format!("{chip_id}-remove"))
                        .label("×")
                        .small()
                        .ghost()
                        .on_click(move |_, _, cx| {
                            shell_remove
                                .update(cx, |shell, cx| shell.remove_chip(field, index, cx));
                        }),
                ),
        );
    }
    row
}

fn keyed_input(
    window: &mut Window,
    cx: &mut App,
    key: String,
    placeholder: &str,
) -> Entity<InputState> {
    window.use_keyed_state(SharedString::from(key), cx, |window, cx| {
        InputState::new(window, cx).placeholder(placeholder.to_owned())
    })
}

fn text_chip_adder(
    shell: Entity<AppShell>,
    id: &'static str,
    placeholder: &'static str,
    kind: ChipAdd,
) -> impl Fn(&mut Window, &mut App, &Entity<AppShell>) -> gpui_kit::AnyElement + 'static {
    move |window, cx, _| {
        let add = keyed_input(window, cx, format!("chips-add-{id}"), placeholder);
        let add_shell = shell.clone();
        let add_input = add.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(Input::new(&add))
            .child(
                Button::new(format!("{id}-add"))
                    .label("Add")
                    .on_click(move |_, window, cx| {
                        let text = add_input.read(cx).value().trim().to_owned();
                        if !text.is_empty() {
                            add_shell.update(cx, |shell, cx| match kind {
                                ChipAdd::ReleaseGroup => shell.add_release_group(&text, cx),
                                ChipAdd::PreferredLanguage => {
                                    shell.add_preferred_language(&text, cx);
                                }
                            });
                            add_input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum ChipAdd {
    ReleaseGroup,
    PreferredLanguage,
}

fn number_chip_adder(
    shell: Entity<AppShell>,
    id: &'static str,
    placeholder: &'static str,
    kind: NumberAdd,
) -> impl Fn(&mut Window, &mut App, &Entity<AppShell>) -> gpui_kit::AnyElement + 'static {
    move |window, cx, _| {
        let add = keyed_input(window, cx, format!("chips-add-{id}"), placeholder);
        let add_shell = shell.clone();
        let add_input = add.clone();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(Input::new(&add))
            .child(
                Button::new(format!("{id}-add"))
                    .label("Add")
                    .on_click(move |_, window, cx| {
                        let text = add_input.read(cx).value().trim().to_owned();
                        if let Ok(value) = text.parse::<u32>() {
                            add_shell.update(cx, |shell, cx| match kind {
                                NumberAdd::Resolution => shell.add_resolution(value, cx),
                            });
                            add_input.update(cx, |input, cx| input.set_value("", window, cx));
                        }
                    }),
            )
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum NumberAdd {
    Resolution,
}

fn codec_adder(
    shell: Entity<AppShell>,
) -> impl Fn(&mut Window, &mut App, &Entity<AppShell>) -> gpui_kit::AnyElement + 'static {
    move |_, cx, _| {
        let present = shell.read(cx).selected_profile().codecs;
        let mut row = div().flex().flex_wrap().gap_1();
        for (codec, label) in CODECS.iter().copied().filter(|(c, _)| !present.contains(c)) {
            let add_shell = shell.clone();
            row = row.child(
                Button::new(SharedString::from(format!("codec-add-{label}")))
                    .label(format!("+ {label}"))
                    .small()
                    .on_click(move |_, _, cx| {
                        add_shell.update(cx, |shell, cx| shell.add_codec(codec, cx));
                    }),
            );
        }
        row.into_any_element()
    }
}

fn source_adder(
    shell: Entity<AppShell>,
) -> impl Fn(&mut Window, &mut App, &Entity<AppShell>) -> gpui_kit::AnyElement + 'static {
    move |_, cx, _| {
        let present = shell.read(cx).selected_profile().sources;
        let mut row = div().flex().flex_wrap().gap_1();
        for (source, label) in SOURCES
            .iter()
            .copied()
            .filter(|(s, _)| !present.contains(s))
        {
            let add_shell = shell.clone();
            row = row.child(
                Button::new(SharedString::from(format!("source-add-{label}")))
                    .label(format!("+ {label}"))
                    .small()
                    .on_click(move |_, _, cx| {
                        add_shell.update(cx, |shell, cx| shell.add_source(source, cx));
                    }),
            );
        }
        row.into_any_element()
    }
}
