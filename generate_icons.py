import re
import os

# Mapping of IconType name to filename (snake_case)
def to_snake_case(name):
    s1 = re.sub('(.)([A-Z][a-z]+)', r'\1_\2', name)
    return re.sub('([a-z0-9])([A-Z])', r'\1_\2', s1).lower()

icon_rs_content = r"""
    match icon {
        // Devices
        IconType::Android => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="5" y="2" width="14" height="20" rx="2" ry="2"/><line x1="12" y1="18" x2="12.01" y2="18"/></svg>"#
        }
        IconType::Ios => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="5" y="2" width="14" height="20" rx="2" ry="2"/><line x1="12" y1="18" x2="12.01" y2="18"/></svg>"#
        }
        IconType::Linux => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="8" r="5"/><path d="M7 13c-1.5 1.5-3 4-3 6 0 1 .5 2 2 2s2.5-1 4-1 2.5 1 4 1 2-1 2-2c0-2-1.5-4.5-3-6"/><path d="M12 13v4"/></svg>"#
        }
        IconType::Windows => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>"#
        }
        IconType::Macos => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>"#
        }
        IconType::Tablet => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="4" y="2" width="16" height="20" rx="2" ry="2"/><line x1="12" y1="18" x2="12.01" y2="18"/></svg>"#
        }
        IconType::Desktop => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="2" y="3" width="20" height="14" rx="2" ry="2"/><line x1="8" y1="21" x2="16" y2="21"/><line x1="12" y1="17" x2="12" y2="21"/></svg>"#
        }
        IconType::Laptop => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M20 16V7a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v9m16 0H4m16 0 1.28 2.55a1 1 0 0 1-.9 1.45H3.62a1 1 0 0 1-.9-1.45L4 16"/></svg>"#
        }
        IconType::Tv => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="2" y="7" width="20" height="15" rx="2" ry="2"/><polyline points="17 2 12 7 7 2"/></svg>"#
        }
        IconType::Watch => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="12" r="7"/><polyline points="12 9 12 12 13.5 13.5"/><path d="M16.51 17.35l-.35 3.83a2 2 0 0 1-2 1.82H9.83a2 2 0 0 1-2-1.82l-.35-3.83m.01-10.7.35-3.83A2 2 0 0 1 9.83 1h4.35a2 2 0 0 1 2 1.82l.35 3.83"/></svg>"#
        }
        IconType::DeviceUnknown => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="12" r="10"/><path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>"#
        }

        // Navigation
        IconType::NavDevices => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="5" y="2" width="14" height="20" rx="2" ry="2"/><line x1="12" y1="18" x2="12.01" y2="18"/></svg>"#
        }
        IconType::NavSettings => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#
        }
        IconType::NavClipboard => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/><rect x="8" y="2" width="8" height="4" rx="1" ry="1"/></svg>"#
        }
        IconType::NavTransfers => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polyline points="17 1 21 5 17 9"/><path d="M3 11V9a4 4 0 0 1 4-4h14"/><polyline points="7 23 3 19 7 15"/><path d="M21 13v2a4 4 0 0 1-4 4H3"/></svg>"#
        }
        IconType::NavFiles => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>"#
        }
        IconType::NavMedia => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="5.5" cy="17.5" r="2.5"/><circle cx="17.5" cy="15.5" r="2.5"/><path d="M8 17V5l12-2v12"/></svg>"#
        }
        IconType::NavPhone => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z"/></svg>"#
        }
        IconType::Folder => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>"#
        }
        IconType::File => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><polyline points="13 2 13 9 20 9"/></svg>"#
        }
        IconType::Send => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><line x1="22" y1="2" x2="11" y2="13"/><polygon points="22 2 15 22 11 13 2 9 22 2"/></svg>"#
        }
        IconType::Receive => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/></svg>"#
        }
        IconType::Download => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>"#
        }
        IconType::Upload => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>"#
        }
        IconType::Sync => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/><path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/></svg>"#
        }
        IconType::Refresh => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polyline points="23 4 23 10 17 10"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></svg>"#
        }
        IconType::Pair => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/></svg>"#
        }
        IconType::Unpair => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M18.84 12.25l1.72-1.71a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M5.17 11.75l-1.71 1.71a5 5 0 0 0 7.07 7.07l1.71-1.71"/><line x1="2" y1="2" x2="22" y2="22"/></svg>"#
        }
        IconType::Block => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="12" r="10"/><line x1="4.93" y1="4.93" x2="19.07" y2="19.07"/></svg>"#
        }
        IconType::Delete => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>"#
        }
        IconType::Settings => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>"#
        }
        IconType::Back => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><line x1="19" y1="12" x2="5" y2="12"/><polyline points="12 19 5 12 12 5"/></svg>"#
        }
        IconType::Close => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>"#
        }
        IconType::Play => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polygon points="5 3 19 12 5 21 5 3"/></svg>"#
        }
        IconType::Pause => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>"#
        }
        IconType::Next => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polygon points="5 4 15 12 5 20 5 4"/><line x1="19" y1="5" x2="19" y2="19"/></svg>"#
        }
        IconType::Previous => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><polygon points="19 20 9 12 19 4 19 20"/><line x1="5" y1="19" x2="5" y2="5"/></svg>"#
        }
        IconType::Music => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><circle cx="5.5" cy="17.5" r="2.5"/><circle cx="17.5" cy="15.5" r="2.5"/><path d="M8 17V5l12-2v12"/></svg>"#
        }
        IconType::Call => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z"/></svg>"#
        }
        IconType::Message => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"/></svg>"#
        }
        IconType::Contact => {
            r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="100%" height="100%"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>"#
        }
    }
"""

def parse_svg_content(svg):
    # This is a very basic parser. It extracts attributes from tags.
    # We need to convert rect, circle, line, polyline, polygon into paths.

    paths = []

    # Clean up svg tag
    inner_content = re.search(r'<svg.*?>(.*)</svg>', svg, re.DOTALL)
    if not inner_content:
        return []

    content = inner_content.group(1)

    # Find all tags
    tags = re.findall(r'<(\w+)\s+(.*?)\s*/?>', content)

    for tag, attrs in tags:
        attr_dict = {}
        for attr in re.findall(r'(\w+)="([^"]*)"', attrs):
            attr_dict[attr[0]] = attr[1]

        if tag == 'path':
            paths.append(attr_dict.get('d', ''))

        elif tag == 'line':
            x1 = attr_dict.get('x1', '0')
            y1 = attr_dict.get('y1', '0')
            x2 = attr_dict.get('x2', '0')
            y2 = attr_dict.get('y2', '0')
            paths.append(f"M {x1} {y1} L {x2} {y2}")

        elif tag == 'polyline':
            points = attr_dict.get('points', '')
            if points:
                pts = points.strip().split()
                # Handle pairs separated by spaces or commas
                # Simplified: assuming standard format like "x1 y1 x2 y2" or "x1,y1 x2,y2"
                # Replacing commas with spaces to be safe
                clean_points = points.replace(',', ' ').split()
                if len(clean_points) >= 2:
                    d = f"M {clean_points[0]} {clean_points[1]}"
                    for i in range(2, len(clean_points), 2):
                        d += f" L {clean_points[i]} {clean_points[i+1]}"
                    paths.append(d)

        elif tag == 'polygon':
            points = attr_dict.get('points', '')
            if points:
                clean_points = points.replace(',', ' ').split()
                if len(clean_points) >= 2:
                    d = f"M {clean_points[0]} {clean_points[1]}"
                    for i in range(2, len(clean_points), 2):
                        d += f" L {clean_points[i]} {clean_points[i+1]}"
                    d += " Z"
                    paths.append(d)

        elif tag == 'rect':
            x = float(attr_dict.get('x', '0'))
            y = float(attr_dict.get('y', '0'))
            w = float(attr_dict.get('width', '0'))
            h = float(attr_dict.get('height', '0'))
            rx = float(attr_dict.get('rx', '0'))
            ry = float(attr_dict.get('ry', '0'))

            if rx == 0 and ry == 0:
                paths.append(f"M {x} {y} h {w} v {h} h {-w} Z")
            else:
                # Rounded rect approximation
                # This is complex to do perfectly in path data string manually,
                # but for simple icons with small corner radius, we can approximate
                # or just use lines and arcs.
                # M x+rx,y H x+w-rx A rx,ry 0 0 1 x+w,y+ry V y+h-ry A rx,ry 0 0 1 x+w-rx,y+h H x+rx A rx,ry 0 0 1 x,y+h-ry V y+ry A rx,ry 0 0 1 x+rx,y
                d = (f"M {x+rx},{y} "
                     f"L {x+w-rx},{y} "
                     f"A {rx},{ry} 0 0 1 {x+w},{y+ry} "
                     f"L {x+w},{y+h-ry} "
                     f"A {rx},{ry} 0 0 1 {x+w-rx},{y+h} "
                     f"L {x+rx},{y+h} "
                     f"A {rx},{ry} 0 0 1 {x},{y+h-ry} "
                     f"L {x},{y+ry} "
                     f"A {rx},{ry} 0 0 1 {x+rx},{y} "
                     f"Z")
                paths.append(d)

        elif tag == 'circle':
            cx = float(attr_dict.get('cx', '0'))
            cy = float(attr_dict.get('cy', '0'))
            r = float(attr_dict.get('r', '0'))

            # Circle using two arcs
            # M cx-r,cy A r,r 0 1,0 cx+r,cy A r,r 0 1,0 cx-r,cy
            d = f"M {cx-r},{cy} A {r},{r} 0 1,0 {cx+r},{cy} A {r},{r} 0 1,0 {cx-r},{cy}"
            paths.append(d)

    return paths

def generate_xml(name, paths):
    filename = f"ic_{to_snake_case(name.replace('IconType::', ''))}.xml"
    path_elements = ""
    for path in paths:
        path_elements += f"""
    <path
        android:pathData="{path}"
        android:strokeColor="#FFFFFF"
        android:strokeWidth="2"
        android:strokeLineCap="round"
        android:strokeLineJoin="round" />"""

    xml_content = f"""<vector xmlns:android="http://schemas.android.com/apk/res/android"
    android:width="24dp"
    android:height="24dp"
    android:viewportWidth="24.0"
    android:viewportHeight="24.0"
    android:tint="?attr/colorControlNormal">
    {path_elements}
</vector>"""

    return filename, xml_content

# Regex to find icon definitions
matches = re.findall(r'(IconType::\w+)\s*=>\s*{\s*r#"(.*?)"#', icon_rs_content, re.DOTALL)

output_dir = "android/app/src/main/res/drawable"
os.makedirs(output_dir, exist_ok=True)

for name, svg in matches:
    paths = parse_svg_content(svg)
    if paths:
        filename, content = generate_xml(name, paths)
        filepath = os.path.join(output_dir, filename)
        with open(filepath, "w") as f:
            f.write(content)
        print(f"Generated {filename}")
