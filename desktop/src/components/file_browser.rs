use crate::components::icon::{Icon, IconType, get_file_icon_type};
use crate::controller::AppAction;
use crate::state::{
    DeviceInfo, LockOrRecover, PreviewData, get_current_remote_files, get_current_remote_path,
    get_preview_data, get_remote_files_update, get_remote_search, get_remote_search_update,
    get_thumbnails, get_thumbnails_update, send_action,
};
use crate::utils::format_file_size;
use base64::Engine as _;
use connected_core::filesystem::{FsEntry, FsEntryType};
use dioxus::prelude::*;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Keys the file browser entries can be sorted by.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SortKey {
    Name,
    Size,
    Date,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Date => "Date",
        }
    }

    fn from_label(s: &str) -> Option<Self> {
        match s {
            "Name" => Some(SortKey::Name),
            "Size" => Some(SortKey::Size),
            "Date" => Some(SortKey::Date),
            _ => None,
        }
    }

    const ALL: [SortKey; 3] = [SortKey::Name, SortKey::Size, SortKey::Date];
}

/// The entries a click action should operate on: deep search results while
/// searching (falling back to the current directory until they arrive),
/// otherwise the plain listing — name-filtered when a query is active.
/// Sorting is applied by the caller since it needs the sort signals.
fn owned_visible_entries(
    files: &Signal<Option<Vec<FsEntry>>>,
    search_results: &Signal<Option<Vec<FsEntry>>>,
    query: &str,
) -> Vec<FsEntry> {
    let mut source = if query.is_empty() {
        files.read().clone().unwrap_or_default()
    } else {
        match search_results.read().as_ref() {
            Some(results) => results.clone(),
            None => files.read().clone().unwrap_or_default(),
        }
    };
    if !query.is_empty() {
        source.retain(|e| e.name.to_lowercase().contains(query));
    }
    source
}

fn toggle_all_visible(
    files: Signal<Option<Vec<FsEntry>>>,
    search_results: Signal<Option<Vec<FsEntry>>>,
    search: Signal<String>,
    sort_key: Signal<SortKey>,
    sort_asc: Signal<bool>,
    mut selected: Signal<HashSet<String>>,
) {
    let query = search.read().trim().to_lowercase();
    let mut vis = owned_visible_entries(&files, &search_results, &query);
    vis.sort_by(|a, b| compare_entries(a, b, *sort_key.read(), *sort_asc.read()));
    let all_now = !vis.is_empty() && vis.iter().all(|e| selected.read().contains(&e.path));
    let mut sel = selected.write();
    if all_now {
        sel.clear();
    } else {
        sel.extend(vis.iter().map(|e| e.path.clone()));
    }
}

fn visible_selected_snapshot(
    files: Signal<Option<Vec<FsEntry>>>,
    search_results: Signal<Option<Vec<FsEntry>>>,
    search: Signal<String>,
    sort_key: Signal<SortKey>,
    sort_asc: Signal<bool>,
    selected: Signal<HashSet<String>>,
) -> Vec<FsEntry> {
    let query = search.read().trim().to_lowercase();
    let mut vis = owned_visible_entries(&files, &search_results, &query);
    let sel = selected.read();
    vis.retain(|e| sel.contains(&e.path));
    drop(sel);
    vis.sort_by(|a, b| compare_entries(a, b, *sort_key.read(), *sort_asc.read()));
    vis
}

/// Directories always come first, then the chosen key. `asc` only flips
/// the chosen key, not the directory grouping.
fn compare_entries(a: &FsEntry, b: &FsEntry, key: SortKey, asc: bool) -> Ordering {
    let dir_a = matches!(a.entry_type, FsEntryType::Directory);
    let dir_b = matches!(b.entry_type, FsEntryType::Directory);
    if dir_a != dir_b {
        return if dir_a {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let ord = match key {
        SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortKey::Size => a.size.cmp(&b.size),
        SortKey::Date => a.modified.unwrap_or(0).cmp(&b.modified.unwrap_or(0)),
    };
    if asc { ord } else { ord.reverse() }
}

#[component]
pub fn FileBrowser(device: DeviceInfo, on_close: EventHandler<()>) -> Element {
    let mut current_path = use_signal(|| get_current_remote_path().lock_or_recover().clone());
    let mut files = use_signal(|| Option::<Vec<FsEntry>>::None);
    let mut loading = use_signal(|| false);
    let mut last_update_seen = use_signal(|| *get_remote_files_update().lock_or_recover());
    let mut context_menu = use_signal(|| Option::<(String, String, i32, i32)>::None);
    let mut preview_content = use_signal(|| Option::<PreviewData>::None);
    let mut video_fullscreen = use_signal(|| false);
    let mut selected = use_signal(HashSet::<String>::new);

    // Search & sort state
    let mut search = use_signal(String::new);
    let mut sort_key = use_signal(|| SortKey::Name);
    let mut sort_asc = use_signal(|| true);
    let mut search_results = use_signal(|| Option::<Vec<FsEntry>>::None);
    let mut last_search_update = use_signal(|| *get_remote_search_update().lock_or_recover());

    // Thumbnail state
    let mut current_thumbnails = use_signal(HashMap::<String, String>::new); // path -> base64
    let mut last_thumbnails_update = use_signal(|| *get_thumbnails_update().lock_or_recover());
    let mut requested_thumbnails = use_signal(HashMap::<String, std::time::Instant>::new);

    use_effect(use_reactive(&device, move |device| {
        loading.set(true);
        send_action(AppAction::ListRemoteFiles {
            ip: device.ip.clone(),
            port: device.port,
            path: "/".to_string(),
        });
    }));

    // Sync state from global stores
    use_future(move || async move {
        loop {
            // Get data from global mutexes
            let (global_update, thumbnails_ts, new_files, new_path, new_preview, search_ts) = {
                let files_update = *get_remote_files_update().lock_or_recover();
                let thumbs_update = *get_thumbnails_update().lock_or_recover();
                let files_list = get_current_remote_files().lock_or_recover().clone();
                let path = get_current_remote_path().lock_or_recover().clone();
                let preview = get_preview_data().lock_or_recover().clone();
                let search_ts = *get_remote_search_update().lock_or_recover();
                (
                    files_update,
                    thumbs_update,
                    files_list,
                    path,
                    preview,
                    search_ts,
                )
            };

            // Update preview if changed
            let current_preview = preview_content.read();
            let should_update_preview = match (current_preview.as_ref(), new_preview.as_ref()) {
                (None, Some(_)) | (Some(_), None) => true,
                (Some(c), Some(n)) => c.filename != n.filename,
                (None, None) => false,
            };
            drop(current_preview);

            if should_update_preview {
                preview_content.set(new_preview);
            }

            // Update files if changed
            if global_update != *last_update_seen.read() {
                files.set(new_files);
                last_update_seen.set(global_update);
                loading.set(false);
                selected.write().clear();
            }

            // Update path if changed
            if new_path != *current_path.read() {
                current_path.set(new_path);
                // Navigating invalidates any active search
                search.set(String::new());
            }

            // Sync search results if updated
            if search_ts != *last_search_update.read() {
                let results = std::mem::take(&mut get_remote_search().lock_or_recover().results);
                search_results.set(results);
                last_search_update.set(search_ts);
            }

            // Sync thumbnails if updated
            if thumbnails_ts != *last_thumbnails_update.read() {
                let thumbs_lock = get_thumbnails().lock_or_recover();
                let mut new_thumbs = HashMap::new();
                for (k, v) in thumbs_lock.iter() {
                    new_thumbs.insert(
                        k.clone(),
                        base64::engine::general_purpose::STANDARD.encode(v),
                    );
                }
                current_thumbnails.set(new_thumbs);
                last_thumbnails_update.set(thumbnails_ts);
            }

            // Allow thumbnail retries by expiring old requests
            {
                let mut requested = requested_thumbnails.write();
                requested.retain(|_, ts| ts.elapsed() < std::time::Duration::from_secs(5));
            }

            // Increased from 200ms to 500ms to reduce CPU usage
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    });

    // Request thumbnails when files change
    let eff_device = device.clone();
    use_effect(move || {
        let entries_opt = files.read();
        if let Some(entries) = entries_opt.as_ref() {
            let thumbs = current_thumbnails.read();
            let mut to_request = Vec::new();

            {
                let requested = requested_thumbnails.read();
                for entry in entries {
                    let is_image = ["jpg", "jpeg", "png", "gif", "webp", "bmp"].contains(
                        &entry
                            .name
                            .split('.')
                            .next_back()
                            .unwrap_or("")
                            .to_lowercase()
                            .as_str(),
                    );
                    if is_image
                        && matches!(entry.entry_type, FsEntryType::File)
                        && !thumbs.contains_key(&entry.path)
                        && !requested.contains_key(&entry.path)
                    {
                        to_request.push(entry.clone());
                    }
                }
            }

            if !to_request.is_empty() {
                let mut requested = requested_thumbnails.write();
                for entry in to_request {
                    requested.insert(entry.path.clone(), std::time::Instant::now());
                    send_action(AppAction::GetThumbnail {
                        ip: eff_device.ip.clone(),
                        port: eff_device.port,
                        path: entry.path,
                    });
                }
            }
        }
    });

    // Auto-hide fullscreen buttons after 500ms of inactivity
    use_effect(move || {
        let has_video = preview_content
            .read()
            .as_ref()
            .is_some_and(|d| d.mime_type.starts_with("video/"));
        if has_video {
            document::eval(
                r#"
                (function() {
                    var timer = null;
                    function getBtns() { return document.querySelectorAll('[data-fullscreen-close], [data-fullscreen-open]'); }
                    function show() {
                        getBtns().forEach(function(b) {
                            b.style.opacity = '1';
                            b.style.pointerEvents = 'auto';
                        });
                        clearTimeout(timer);
                        timer = setTimeout(function() {
                            getBtns().forEach(function(b) {
                                b.style.opacity = '0';
                                b.style.pointerEvents = 'none';
                            });
                        }, 500);
                    }
                    show();
                    window.__connected_fs_handler = function() { show(); };
                    window.addEventListener('mousemove', window.__connected_fs_handler);
                    window.__connected_fs_keydown = function(e) {
                        if (e.key === 'Escape') {
                            var close = document.querySelector('[data-fullscreen-close]');
                            if (close) close.click();
                        }
                    };
                    window.addEventListener('keydown', window.__connected_fs_keydown);
                })();
            "#,
            );
        } else {
            document::eval(
                r#"
                if (window.__connected_fs_handler) {
                    window.removeEventListener('mousemove', window.__connected_fs_handler);
                    window.__connected_fs_handler = null;
                }
                if (window.__connected_fs_keydown) {
                    window.removeEventListener('keydown', window.__connected_fs_keydown);
                    window.__connected_fs_keydown = null;
                }
            "#,
            );
        }
    });

    // Dispatch a debounced recursive search when the query changes
    let search_device = device.clone();
    use_effect(move || {
        let query = search.read().trim().to_string();
        let ip = search_device.ip.clone();
        let port = search_device.port;
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            // Superseded by newer keystrokes — skip
            if search.read().trim() != query {
                return;
            }
            if query.is_empty() {
                let mut state = get_remote_search().lock_or_recover();
                state.request_id += 1;
                state.results = None;
                drop(state);
                search_results.set(None);
                return;
            }
            let path = current_path.read().clone();
            let request_id = {
                let mut state = get_remote_search().lock_or_recover();
                state.request_id += 1;
                state.results = None;
                state.request_id
            };
            search_results.set(None);
            send_action(AppAction::SearchRemote {
                ip,
                port,
                path,
                query,
                request_id,
            });
        });
    });

    // Read signals once at the top of render to minimize borrow time
    let current_path_val = current_path.read();
    let files_val = files.read();
    let loading_val = *loading.read();
    let thumbnails_val = current_thumbnails.read();
    let preview_val = preview_content.read();
    let context_menu_val = context_menu.read();
    let selected_val = selected.read();
    let search_val = search.read();
    let sort_key_val = *sort_key.read();
    let sort_asc_val = *sort_asc.read();
    let search_results_val = search_results.read();

    // The visible listing: deep search results while searching (falling back
    // to the current directory until they arrive), otherwise the plain listing.
    let query = search_val.trim().to_lowercase();
    let searching = !query.is_empty();
    let source_entries: Option<&Vec<FsEntry>> = if searching {
        search_results_val.as_ref().or(files_val.as_ref())
    } else {
        files_val.as_ref()
    };
    let mut visible: Vec<&FsEntry> = source_entries
        .map(|entries| {
            entries
                .iter()
                .filter(|e| e.name.to_lowercase().contains(query.as_str()))
                .collect()
        })
        .unwrap_or_default();
    visible.sort_by(|a, b| compare_entries(a, b, sort_key_val, sort_asc_val));

    let show_selection_bar =
        !loading_val && (searching || files_val.as_ref().is_some_and(|e| !e.is_empty()));
    let selected_count = visible
        .iter()
        .filter(|e| selected_val.contains(&e.path))
        .count();
    let all_selected = !visible.is_empty() && visible.len() == selected_count;

    rsx! {
        div {
            class: "file-browser",
            onclick: move |_| context_menu.set(None),

            div {
                class: "browser-header",
                button {
                    class: "secondary-button",
                    onclick: {
                        let ip = device.ip.clone();
                        let port = device.port;
                        let p = current_path_val.clone();
                        move |_| {
                            if p != "/" {
                                let parent = std::path::Path::new(&p)
                                    .parent()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or("/".to_string());
                                let parent = if parent.is_empty() {
                                    "/".to_string()
                                } else {
                                    parent
                                };

                                loading.set(true);
                                send_action(AppAction::ListRemoteFiles {
                                    ip: ip.clone(),
                                    port,
                                    path: parent,
                                });
                            } else {
                                on_close.call(());
                            }
                        }
                    },
                    Icon { icon: IconType::Back, size: 14, color: "currentColor".to_string() }
                    span { " Back" }
                }
                h3 { "Files on {device.name}" }
                span { class: "path-display", "{current_path_val}" }
            }

            if show_selection_bar {
                div {
                    class: "selection-bar",
                    span {
                        class: "selection-toggle",
                        onclick: move |_| {
                            toggle_all_visible(
                                files,
                                search_results,
                                search,
                                sort_key,
                                sort_asc,
                                selected,
                            )
                        },
                        SelectionCheckbox {
                            checked: all_selected,
                            partial: selected_count > 0 && !all_selected,
                            on_toggle: move |_| {
                                toggle_all_visible(
                                    files,
                                    search_results,
                                    search,
                                    sort_key,
                                    sort_asc,
                                    selected,
                                )
                            },
                        }
                        span { class: "selection-label", "Select all" }
                    }
                    input {
                        class: "search-input",
                        r#type: "text",
                        placeholder: "Search current folder and subfolders...",
                        value: "{search_val}",
                        oninput: move |e| search.set(e.value()),
                    }
                    select {
                        class: "sort-select",
                        title: "Sort entries by",
                        value: "{sort_key_val:?}",
                        onchange: move |e| {
                            if let Some(key) = SortKey::from_label(&e.value()) {
                                sort_key.set(key);
                            }
                        },
                        for key in SortKey::ALL {
                            option { value: "{key:?}", "{key.label()}" }
                        }
                    }
                    button {
                        class: "dir-btn",
                        title: if sort_asc_val { "Ascending" } else { "Descending" },
                        onclick: move |_| sort_asc.toggle(),
                        style: if sort_asc_val {
                            "transform: rotate(-90deg);"
                        } else {
                            "transform: rotate(90deg);"
                        },
                        Icon { icon: IconType::ArrowRight, size: 14, color: "currentColor".to_string() }
                    }
                    if selected_count > 0 {
                        span { class: "selection-count", "{selected_count} selected" }
                        button {
                            class: "secondary-button",
                            onclick: {
                                let ip = device.ip.clone();
                                let port = device.port;
                                move |_| {
                                    let chosen = visible_selected_snapshot(
                                        files,
                                        search_results,
                                        search,
                                        sort_key,
                                        sort_asc,
                                        selected,
                                    );
                                    if !chosen.is_empty() {
                                        send_action(AppAction::DownloadEntries {
                                            ip: ip.clone(),
                                            port,
                                            entries: chosen,
                                        });
                                    }
                                    selected.write().clear();
                                }
                            },
                            Icon { icon: IconType::Download, size: 14, color: "currentColor".to_string() }
                            span { " Download" }
                        }
                        button {
                            class: "secondary-button",
                            onclick: move |_| selected.write().clear(),
                            Icon { icon: IconType::Close, size: 12, color: "currentColor".to_string() }
                            span { " Clear" }
                        }
                    }
                }
            }

            if loading_val {
                div {
                    class: "loading",
                    div { class: "searching-indicator",
                        span { class: "dot" }
                        span { class: "dot" }
                        span { class: "dot" }
                    }
                    span { "Loading files..." }
                }
            } else if files_val.is_some() || searching {
                div {
                    class: "file-list",
                    if searching && search_results_val.is_none() {
                        div { class: "search-status", "Searching for \"{search_val}\"..." }
                    }
                    if searching
                        && search_results_val.as_ref().is_some_and(|r| r.is_empty())
                    {
                        div { class: "search-status", "No matches" }
                    }
                    if !searching && current_path_val.as_str() != "/" {
                        div {
                            class: "file-entry directory",
                            onclick: {
                                let ip = device.ip.clone();
                                let port = device.port;
                                let p = current_path_val.clone();
                                move |_| {
                                    if p != "/" {
                                        let parent = std::path::Path::new(&p)
                                            .parent()
                                            .map(|p| p.to_string_lossy().to_string())
                                            .unwrap_or("/".to_string());
                                        let parent = if parent.is_empty() {
                                            "/".to_string()
                                        } else {
                                            parent
                                        };

                                        loading.set(true);
                                        send_action(AppAction::ListRemoteFiles {
                                            ip: ip.clone(),
                                            port,
                                            path: parent,
                                        });
                                    }
                                }
                            },
                            span { class: "checkbox-spacer" }
                            span {
                                class: "icon",
                                Icon { icon: IconType::Folder, size: 18, color: "var(--accent)".to_string() }
                            }
                            span { class: "name", ".." }
                            span { class: "size", "" }
                        }
                    }
                    for entry in visible {
                        {
                            let rel_parent: &str = if searching {
                                entry.path
                                    .strip_prefix(current_path_val.trim_end_matches('/'))
                                    .unwrap_or(&entry.path)
                                    .trim_start_matches('/')
                                    .rsplit_once('/')
                                    .map(|(d, _)| d)
                                    .unwrap_or("")
                            } else {
                                ""
                            };
                            let entry_class = match entry.entry_type {
                                FsEntryType::Directory => "file-entry directory",
                                _ => "file-entry file",
                            };
                            let entry_checked = selected_val.contains(&entry.path);
                            let selected_suffix =
                                if entry_checked { " selected" } else { "" };
                            let icon_type = match entry.entry_type {
                                FsEntryType::Directory => IconType::Folder,
                                _ => get_file_icon_type(&entry.name),
                            };
                            let icon_color = match entry.entry_type {
                                FsEntryType::Directory => "var(--accent)",
                                _ => "var(--text-secondary)",
                            };
                            let entry_path_for_toggle = entry.path.clone();

                            rsx! {
                                div {
                                    class: "{entry_class}{selected_suffix}",
                                    onclick: {
                                        let entry = entry.clone();
                                        let ip = device.ip.clone();
                                        let port = device.port;
                                        move |_evt: Event<MouseData>| {
                                            if let FsEntryType::Directory = entry.entry_type {
                                                loading.set(true);
                                                send_action(AppAction::ListRemoteFiles {
                                                    ip: ip.clone(),
                                                    port,
                                                    path: entry.path.clone(),
                                                });
                                            } else {
                                                send_action(AppAction::PreviewFile {
                                                    ip: ip.clone(),
                                                    port,
                                                    remote_path: entry.path.clone(),
                                                    filename: entry.name.clone(),
                                                });
                                            }
                                        }
                                    },
                                    oncontextmenu: {
                                        let entry = entry.clone();
                                        move |evt: Event<MouseData>| {
                                            evt.prevent_default();
                                            if let FsEntryType::File = entry.entry_type {
                                                let coords = evt.client_coordinates();
                                                context_menu.set(Some((
                                                    entry.path.clone(),
                                                    entry.name.clone(),
                                                    coords.x as i32,
                                                    coords.y as i32
                                                )));
                                            }
                                        }
                                    },
                                    SelectionCheckbox {
                                        checked: entry_checked,
                                        partial: false,
                                        on_toggle: move |_| {
                                            let mut sel = selected.write();
                                            if !sel.remove(&entry_path_for_toggle) {
                                                sel.insert(entry_path_for_toggle.clone());
                                            }
                                        },
                                    },
                                    span {
                                        class: "icon",
                                        if let Some(thumbnail_data) = thumbnails_val.get(&entry.path) {
                                            img {
                                                src: "data:image/jpeg;base64,{thumbnail_data}",
                                                style: "width: 24px; height: 24px; object-fit: cover; border-radius: 4px; display: block;"
                                            }
                                        } else {
                                            Icon { icon: icon_type, size: 18, color: icon_color.to_string() }
                                        }
                                    }
                                    span {
                                        class: "name",
                                        "{entry.name}"
                                        if !rel_parent.is_empty() {
                                            span { class: "entry-path", " · {rel_parent}" }
                                        }
                                    }
                                    span { class: "size", "{format_file_size(entry.size)}" }
                                }
                            }
                        }
                    }
                }
            } else {
                div {
                    class: "empty",
                    Icon { icon: IconType::Folder, size: 48, color: "var(--text-tertiary)".to_string() }
                    p { "No files found or connection error" }
                }
            }

            if let Some((path, name, x, y)) = context_menu_val.as_ref() {
                div {
                    class: "context-menu",
                    style: "top: {y}px; left: {x}px;",
                    div {
                        class: "menu-item",
                        onclick: {
                            let ip = device.ip.clone();
                            let port = device.port;
                            let path = path.clone();
                            let name = name.clone();
                            move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                context_menu.set(None);
                                send_action(AppAction::DownloadFile {
                                    ip: ip.clone(),
                                    port,
                                    remote_path: path.clone(),
                                    filename: name.clone(),
                                });
                            }
                        },
                        Icon { icon: IconType::Download, size: 14, color: "currentColor".to_string() }
                        span { " Download" }
                    }
                }
            }

            if *video_fullscreen.read() && preview_val.as_ref().is_some_and(|d| d.mime_type.starts_with("video/")) {
                div {
                    style: "position: fixed; inset: 0; z-index: 3000; background: #000; display: flex; align-items: center; justify-content: center;",
                    onclick: move |_| video_fullscreen.set(false),
                    video {
                        controls: "true",
                        src: "data:{preview_val.as_ref().unwrap().mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&preview_val.as_ref().unwrap().data)}",
                        style: "max-width: 100vw; max-height: 100vh; width: 100vw; height: 100vh; object-fit: contain;",
                        onclick: |evt| evt.stop_propagation(),
                        onloadeddata: move |_| {
                            document::eval(r#"
                                var v = document.querySelector('[data-fullscreen-close]').parentElement.querySelector('video');
                                if (v && window.__connected_fs_time !== undefined) {
                                    v.currentTime = window.__connected_fs_time;
                                    if (!window.__connected_fs_paused) v.play();
                                }
                            "#);
                        },
                    }
                    button {
                        "data-fullscreen-close": "true",
                        style: "position: absolute; top: 16px; left: 50%; transform: translateX(-50%); z-index: 10; background: rgba(0,0,0,0.8); color: white; border: none; border-radius: 50%; width: 40px; height: 40px; display: flex; align-items: center; justify-content: center; cursor: pointer; transition: opacity 0.3s;",
                        onclick: move |evt| {
                            evt.stop_propagation();
                            document::eval(r#"
                                var v = document.querySelector('[data-fullscreen-close]').parentElement.querySelector('video');
                                if (v) {
                                    window.__connected_fs_paused = v.paused;
                                    window.__connected_fs_time = v.currentTime;
                                    v.pause();
                                }
                            "#);
                            video_fullscreen.set(false);
                            dioxus::desktop::window().window.set_fullscreen(None);
                        },
                        Icon { icon: IconType::Close, size: 20, color: "white".to_string() }
                    }
                }
            } else if let Some(data) = preview_val.as_ref() {
                div {
                    class: "modal-overlay",
                    onclick: move |_| send_action(AppAction::ClosePreview),
                    div {
                        class: "modal-content",
                        style: "max-width: 90vw; max-height: 90vh; overflow: hidden;",
                        onclick: |evt| evt.stop_propagation(),

                        div {
                            class: "dialog-header",
                            h2 { "{data.filename}" }
                            button {
                                class: "dialog-close",
                                onclick: move |_| send_action(AppAction::ClosePreview),
                                Icon { icon: IconType::Close, size: 16, color: "currentColor".to_string() }
                            }
                        }

                        div {
                            class: "dialog-content",
                            style: "overflow: auto; max-height: 70vh; display: flex; align-items: center; justify-content: center;",
                            if data.mime_type.starts_with("image/") {
                                img {
                                    src: "data:{data.mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&data.data)}",
                                    style: "max-width: 100%; max-height: 65vh; object-fit: contain; border-radius: 8px;"
                                }
                            } else if data.mime_type.starts_with("text/") {
                                pre {
                                    style: "white-space: pre-wrap; font-family: var(--font-mono); text-align: left; padding: 16px; background: var(--bg-tertiary); border-radius: 8px; width: 100%; overflow-x: auto;",
                                    "{String::from_utf8_lossy(&data.data)}"
                                }
                            } else if data.mime_type.starts_with("audio/") {
                                audio {
                                    controls: "true",
                                    src: "data:{data.mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&data.data)}",
                                    style: "max-width: 100%; border-radius: 8px;"
                                }
                            } else if data.mime_type.starts_with("video/") {
                                div {
                                    style: "position: relative; width: fit-content; margin: 0 auto;",
                                    video {
                                        controls: "true",
                                        src: "data:{data.mime_type};base64,{base64::engine::general_purpose::STANDARD.encode(&data.data)}",
                                        style: "max-width: 100%; max-height: 65vh; border-radius: 8px;",
                                        onloadeddata: move |_| {
                                            document::eval(r#"
                                                var v = document.querySelector('.dialog-content video');
                                                if (v && window.__connected_fs_time !== undefined) {
                                                    v.currentTime = window.__connected_fs_time;
                                                    if (!window.__connected_fs_paused) v.play();
                                                }
                                            "#);
                                        },
                                    }
                                    button {
                                        "data-fullscreen-open": "true",
                                        style: "position: absolute; top: 8px; left: 50%; transform: translateX(-50%); z-index: 10; background: rgba(0,0,0,0.6); color: white; border: none; border-radius: 50%; width: 36px; height: 36px; display: flex; align-items: center; justify-content: center; cursor: pointer; backdrop-filter: blur(4px); transition: opacity 0.3s;",
                                        onclick: move |evt| {
                                            evt.stop_propagation();
                                            document::eval(r#"
                                                var v = document.querySelector('.dialog-content video');
                                                if (v) {
                                                    window.__connected_fs_paused = v.paused;
                                                    window.__connected_fs_time = v.currentTime;
                                                    v.pause();
                                                }
                                            "#);
                                            video_fullscreen.set(true);
                                            dioxus::desktop::window()
                                                .window
                                                .set_fullscreen(Some(
                                                    dioxus::desktop::tao::window::Fullscreen::Borderless(None),
                                                ));
                                        },
                                        Icon { icon: IconType::Fullscreen, size: 18, color: "white".to_string() }
                                    }
                                }
                            } else {
                                div {
                                    class: "empty-state",
                                    Icon { icon: IconType::File, size: 48, color: "var(--text-tertiary)".to_string() }
                                    p { "Preview not available for {data.mime_type}" }
                                    p { class: "muted", "Size: {format_file_size(data.data.len() as u64)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SelectionCheckbox(checked: bool, partial: bool, on_toggle: EventHandler<()>) -> Element {
    let class = if checked {
        "checkbox checked"
    } else if partial {
        "checkbox partial"
    } else {
        "checkbox"
    };

    rsx! {
        button {
            class: "{class}",
            title: "Select",
            onclick: move |evt: Event<MouseData>| {
                evt.stop_propagation();
                on_toggle.call(());
            },
            if checked {
                Icon { icon: IconType::Check, size: 12, color: "currentColor".to_string() }
            } else if partial {
                span { class: "checkbox-dash" }
            }
        }
    }
}
