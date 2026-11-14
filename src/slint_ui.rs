use crate::json::serialize_flight_trace_json;
use crate::model::SystemSnapshot;
use crate::providers::LinuxSystemSampler;
use crate::providers::processes::{ProcessAction, control_process, inspect_process};
use crate::providers::system_catalog::{format_bytes, format_system_info_report};
use crate::settings::{AppSettings, ThemeColors, parse_hex_color, rgb_to_hex};
use anyhow::Result;
use slint::{Color, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::slint! {
    import "../assets/fonts/DejaVuSansMono.ttf";
    import "../assets/fonts/NotoSans-Regular.ttf";
    import { ScrollView, LineEdit } from "std-widgets.slint";

    export struct ProcRow {
        pid: int,
        ppid: int,
        name: string,
        display_name: string,
        user: string,
        state: string,
        cpu_str: string,
        gpu_str: string,
        mem_pct_str: string,
        rss_str: string,
        io_read_str: string,
        io_write_str: string,
        threads: int,
        nice: int,
        cmdline: string,
        highlight_kind: int, // 0=none, 1=green, 2=red
    }

    export struct PerfNavItem {
        index: int,
        icon: string,
        icon_color: color,
        title: string,
        sub1: string,
        sub2: string,
        sparkline: image,
    }

    export struct CorePlotData {
        core-idx: int,
        label: string,
        stroke-path: string,
        fill-path: string,
        kernel-path: string,
        val-str: string,
    }

    export struct ConnRow {
        proto: string,
        local_ep: string,
        remote_ep: string,
        state: string,
        pid_str: string,
        proc_name: string,
        public_badge: string,
        is_public: bool,
    }

    export struct SensorRow {
        chip: string,
        adapter: string,
        label: string,
        category: string,
        cur_str: string,
        min_str: string,
        max_str: string,
        ratio: float,
    }

    export struct DiskUsageRow {
        rank: string,
        name: string,
        path: string,
        size_str: string,
        files_str: string,
        pct_str: string,
        ratio: float,
    }

    export struct DriverRow {
        name: string,
        size_str: string,
        refs_str: string,
        state: string,
        in_tree: string,
        used_by: string,
        path: string,
    }

    export struct CatalogRow {
        col1: string,
        col2: string,
        col3: string,
        col4: string,
        col5: string,
        col6: string,
    }

    // Sub-sample scroll position shared by all live charts (0 = newest sample just
    // arrived and is still off-screen right, 1 = scrolled fully into view).
    export global ChartClock {
        in-out property <float> phase: 1.0;
    }

    export global Theme {
        // Unlit dot-matrix character cells behind VFD banner text (rendered at 2x).
        in-out property <image> lcd-ghost;
        in-out property <color> bg-main: #090c0e;
        in-out property <color> bg-panel: #0d1015;
        in-out property <color> bg-card: #0c0f14;
        in-out property <color> bg-header: #11161d;
        in-out property <color> border: #1e2631;
        in-out property <color> text-main: #e6edf3;
        in-out property <color> text-dim: #7d8998;
        in-out property <color> accent-primary: #22e58b;
        in-out property <color> accent-secondary: #19b5fe;
        in-out property <color> accent-warn: #f5a623;
        in-out property <color> accent-danger: #ff4757;
        in-out property <color> accent-purple: #b84dff;
        in-out property <color> row-alt: #0f141a;
        in-out property <color> nav-selected: #132c47;
        in-out property <color> nav-border: #1e5785;
        in-out property <color> menu-bg: #10151d;
        in-out property <color> menu-border: #1e2631;
        in-out property <color> menu-hover: #19222d;
        in-out property <string> font-family: "DejaVu Sans Mono";
        in-out property <string> font-ui: "sans-serif";
        in-out property <length> font-size-sm: 11px;
        in-out property <length> font-size-base: 12px;
        in-out property <length> font-size-md: 14px;
        in-out property <length> font-size-lg: 18px;
        in-out property <length> font-size-xl: 22px;
        in-out property <length> row-height: 24px;
        in-out property <bool> show-grid: true;

        // Visual Toolkit Style:
        // 0 = Retro VFD / Hardware Terminal
        // 1 = Fluent Design (Windows 11 Task Manager Mica & soft curves)
        // 2 = KDE Plasma 6 Breeze (Qt Breeze frames, accents & clean tabs)
        in-out property <int> ui-style: 0;
        in-out property <length> card-radius: 6px;
        in-out property <length> button-radius: 4px;
    }

    // Left Sidebar Navigation Item
    component SidebarItem inherits Rectangle {
        in property <string> icon;
        in property <string> label;
        in property <bool> active: false;
        callback clicked();

        height: 32px;
        border-radius: Theme.button-radius;
        background: root.active ? Theme.nav-selected : (ta.has-hover ? Theme.menu-hover : transparent);
        border-width: Theme.ui-style == 2 ? (root.active ? 1px : 0px) : (root.active ? 1px : 0px);
        border-color: Theme.nav-border;

        // Windows 11 Fluent Active Indicator: 3px rounded vertical accent pill on the left!
        if Theme.ui-style == 1 && root.active : Rectangle {
            x: 0px;
            y: 7px;
            width: 3px;
            height: 18px;
            border-radius: 1.5px;
            background: Theme.accent-primary;
        }

        // KDE Breeze Active Indicator: Breeze Blue left accent line
        if Theme.ui-style == 2 && root.active : Rectangle {
            x: 0px;
            y: 0px;
            width: 3px;
            height: parent.height;
            background: Theme.accent-primary;
        }

        HorizontalLayout {
            padding-left: Theme.ui-style == 1 ? 12px : 10px;
            padding-right: 10px;
            spacing: 10px;
            Text {
                width: 18px;
                text: root.icon;
                color: root.active ? Theme.accent-primary : (ta.has-hover ? Theme.text-main : Theme.text-dim);
                font-family: "DejaVu Sans Mono";
                font-size: 14px;
                vertical-alignment: center;
                horizontal-alignment: center;
            }
            Text {
                text: root.label;
                color: root.active ? #ffffff : Theme.text-main;
                font-family: Theme.font-ui;
                font-size: Theme.font-size-base;
                font-weight: root.active ? 600 : 400;
                vertical-alignment: center;
            }
        }
        ta := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    // Recessed VFD / Section Header banner used on Summary cards
    component VfdBanner inherits Rectangle {
        in property <string> left-text;
        in property <string> right-text: "";
        height: 30px;
        background: Theme.ui-style == 1 ? #ffffff08 : (Theme.ui-style == 2 ? Theme.bg-header : #060f12);
        border-width: 1px;
        border-color: Theme.ui-style == 1 ? Theme.border : (Theme.ui-style == 2 ? Theme.border : #1d3940);
        border-radius: Theme.card-radius;

        // --- Retro VFD style: character-cell LCD with unlit dot-matrix cells ---
        // Glyph grid = DejaVu Sans Mono 15px advance (9.03px) + 1.5px letter spacing.
        property <length> lcd-pitch: 10.5308px;
        property <length> lcd-pad: 10px;
        property <int> lcd-cells: floor((root.width - 2 * root.lcd-pad) / root.lcd-pitch);

        if Theme.ui-style == 0 : Rectangle {
            x: root.lcd-pad;
            y: 0px;
            width: root.lcd-cells * root.lcd-pitch;
            height: root.height;
            clip: true;
            Image {
                x: 0px;
                y: (parent.height - self.height) / 2;
                width: Theme.lcd-ghost.width * 0.5px;
                height: Theme.lcd-ghost.height * 0.5px;
                source: Theme.lcd-ghost;
                image-fit: fill;
            }
        }

        if Theme.ui-style == 0 : Text {
            x: root.lcd-pad;
            height: root.height;
            text: root.left-text;
            color: #36e8ff;
            font-family: "DejaVu Sans Mono";
            font-size: 15px;
            letter-spacing: 1.5px;
            vertical-alignment: center;
        }

        if Theme.ui-style == 0 && root.right-text != "" : rt := Text {
            // Snap right-aligned text onto the same character grid.
            property <int> chars: round((self.preferred-width + 1.5px) / root.lcd-pitch);
            x: root.lcd-pad + max(0, root.lcd-cells - self.chars) * root.lcd-pitch;
            height: root.height;
            text: root.right-text;
            color: #36e8ff;
            font-family: "DejaVu Sans Mono";
            font-size: 15px;
            letter-spacing: 1.5px;
            vertical-alignment: center;
        }

        // KDE Breeze Accent Top Line
        if Theme.ui-style == 2 : Rectangle {
            x: 0px;
            y: 0px;
            width: parent.width;
            height: 2px;
            background: Theme.accent-primary;
        }

        if Theme.ui-style != 0 : HorizontalLayout {
            padding-left: 12px;
            padding-right: 12px;
            alignment: space-between;

            HorizontalLayout {
                spacing: 8px;
                alignment: start;
                // Fluent Modern accent dot
                if Theme.ui-style == 1 : Rectangle {
                    width: 6px;
                    height: 6px;
                    border-radius: 3px;
                    background: Theme.accent-primary;
                    y: (parent.height - self.height) / 2;
                }
                Text {
                    text: root.left-text;
                    color: Theme.text-main;
                    font-family: Theme.font-ui;
                    font-size: 13px;
                    font-weight: 700;
                    letter-spacing: 0.5px;
                    vertical-alignment: center;
                }
            }
            if root.right-text != "" : Text {
                text: root.right-text;
                color: Theme.text-dim;
                font-family: Theme.font-ui;
                font-size: 12px;
                font-weight: 500;
                vertical-alignment: center;
            }
        }
    }

    component ActionBtn inherits Rectangle {
        in property <string> label;
        in property <color> stroke-col: Theme.border;
        in property <bool> filled: false;
        callback clicked();

        height: 26px;
        min-width: max(26px, btn-lbl.preferred-width + 18px);
        border-radius: Theme.button-radius;
        background: root.filled ? Theme.accent-primary : (ta.pressed ? Theme.nav-selected : (ta.has-hover ? Theme.menu-hover : Theme.bg-card));
        border-width: 1px;
        border-color: root.filled ? Theme.accent-primary : (ta.has-hover ? Theme.accent-secondary : root.stroke-col);
        drop-shadow-blur: Theme.ui-style == 1 && ta.has-hover ? 4px : 0px;
        drop-shadow-color: #00000044;

        HorizontalLayout {
            padding-left: 8px;
            padding-right: 8px;
            alignment: center;
            btn-lbl := Text {
                text: root.label;
                color: root.filled ? #000000 : (ta.has-hover ? #ffffff : Theme.text-main);
                font-family: Theme.font-ui;
                font-size: Theme.font-size-sm;
                font-weight: 500;
                vertical-alignment: center;
                horizontal-alignment: center;
            }
        }
        ta := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
        }
    }

    component BarMeter inherits Rectangle {
        in property <float> ratio: 0.0;
        in property <color> fill-color: Theme.accent-primary;
        height: 6px;
        border-radius: 3px;
        background: #06090c;
        border-width: 1px;
        border-color: Theme.border;
        clip: true;

        Rectangle {
            x: 0px;
            y: 0px;
            height: parent.height;
            width: parent.width * clamp(root.ratio, 0.0, 1.0);
            background: root.fill-color;
        }
    }

    component MenuBarButton inherits Rectangle {
        in property <string> label;
        in property <bool> active: false;
        callback clicked();
        callback hovered();

        height: 24px;
        border-radius: 4px;
        background: root.active ? Theme.nav-selected : (ta.has-hover ? Theme.menu-hover : transparent);
        border-width: root.active ? 1px : 0px;
        border-color: Theme.nav-border;

        HorizontalLayout {
            padding-left: 9px;
            padding-right: 9px;
            alignment: center;
            Text {
                text: root.label;
                color: root.active ? #ffffff : Theme.text-main;
                font-family: Theme.font-ui;
                font-size: Theme.font-size-base;
                font-weight: root.active ? 600 : 400;
                vertical-alignment: center;
            }
        }
        ta := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.clicked(); }
            moved => {
                if (ta.has-hover) {
                    root.hovered();
                }
            }
        }
    }

    component MenuDropdownContainer inherits Rectangle {
        in property <length> menu-width: 280px;
        width: root.menu-width;
        height: vl.preferred-height;
        background: Theme.menu-bg;
        border-width: 1px;
        border-color: Theme.menu-border;
        border-radius: 8px;
        drop-shadow-blur: 16px;
        drop-shadow-color: #000000aa;
        drop-shadow-offset-y: 5px;

        vl := VerticalLayout {
            padding: 5px;
            spacing: 2px;
            @children
        }
    }

    component MenuDropdownItem inherits Rectangle {
        in property <string> icon: "";
        in property <string> label;
        in property <string> shortcut: "";
        callback triggered();

        height: 29px;
        border-radius: 5px;
        background: ta.has-hover ? Theme.menu-hover : transparent;
        border-width: ta.has-hover ? 1px : 0px;
        border-color: Theme.nav-border;

        HorizontalLayout {
            padding-left: 7px;
            padding-right: 9px;
            alignment: space-between;

            HorizontalLayout {
                spacing: 7px;
                alignment: start;
                Rectangle {
                    width: 18px;
                    height: 18px;
                    if root.icon != "" : Text {
                        text: root.icon;
                        color: ta.has-hover ? Theme.accent-primary : Theme.accent-secondary;
                        font-size: 13px;
                        font-family: Theme.font-ui;
                        horizontal-alignment: center;
                        vertical-alignment: center;
                    }
                }
                Text {
                    text: root.label;
                    color: ta.has-hover ? #ffffff : Theme.text-main;
                    font-size: Theme.font-size-base;
                    font-family: Theme.font-ui;
                    vertical-alignment: center;
                }
            }
            if root.shortcut != "" : Text {
                text: root.shortcut;
                color: ta.has-hover ? Theme.accent-primary : Theme.text-dim;
                font-size: Theme.font-size-sm;
                font-family: Theme.font-ui;
                vertical-alignment: center;
            }
        }
        ta := TouchArea {
            mouse-cursor: pointer;
            clicked => { root.triggered(); }
        }
    }

    component MenuDivider inherits Rectangle {
        height: 7px;
        VerticalLayout {
            alignment: center;
            Rectangle {
                height: 1px;
                background: Theme.menu-border;
                opacity: 0.7;
            }
        }
    }

    component DashedVLine inherits Rectangle {
        y: 0px;
        width: 1px;
        height: 100%;
        for i in floor(root.height / 6px) : Rectangle {
            x: 0px;
            y: i * 6px;
            width: 1px;
            height: 3px;
            background: #e6f7ff;
            opacity: 0.85;
        }
    }

    component VectorTelemetryChart inherits Rectangle {
        // Number of points in the series ring (one is kept off-screen for scrolling).
        in property <int> points: 120;
        // Visible window covers points-2 intervals; it slides by one interval per sample.
        property <float> vb-interval: 1000 / max(1, root.points - 1);
        property <float> vb-x: ChartClock.phase * root.vb-interval;
        property <float> vb-w: 1000 - root.vb-interval;
        in property <string> stroke-path: "";
        in property <string> fill-path: "";
        in property <string> kernel-path: "";
        in property <string> temp-path: "";
        in property <color> stroke-color: #36c5ff;
        in property <color> fill-color: #36c5ff22;
        in property <color> kernel-color: #ff5566;
        in property <color> temp-color: #f5a623;
        in property <bool> has-kernel: false;
        in property <bool> has-temp: false;
        in property <bool> show-grid: true;
        in property <string> max-label: "100%";
        in property <string> min-label: "0%";
        in property <string> time-label: "60 seconds";
        in property <string> title-label: "% Utilization";
        in property <string> current-val-label: "";

        // Optional right-hand axis (e.g. temperature scale on the CPU history chart).
        in property <string> right-max-label: "";
        in property <string> right-mid-label: "";
        in property <string> right-min-label: "";

        // Optional range selection: drag to select, click to clear. sel-a/sel-b are
        // data-space fractions (0 = oldest point, 1 = newest) so the band scrolls
        // together with the curves.
        in property <bool> selectable: false;
        in property <bool> sel-active: false;
        in property <float> sel-a: 0;
        in property <float> sel-b: 0;
        callback range-selected(float, float); // screen-space fractions of chart width
        callback range-cleared();
        function data-to-x(u: float) -> length {
            return (u * 1000 - root.vb-x) / root.vb-w * root.width;
        }

        background: Theme.ui-style == 1 ? #00000022 : (Theme.ui-style == 2 ? Theme.bg-card : #04080b);
        border-width: 1px;
        border-color: Theme.border;
        border-radius: Theme.card-radius;
        clip: true;

        // Grid lines (Crisp vector rendering)
        if root.show-grid : Rectangle {
            width: 100%;
            height: 100%;

            // Horizontal lines
            Rectangle { y: parent.height * 0.25; width: 100%; height: 1px; background: Theme.border; opacity: 0.35; }
            Rectangle { y: parent.height * 0.50; width: 100%; height: 1px; background: Theme.border; opacity: 0.35; }
            Rectangle { y: parent.height * 0.75; width: 100%; height: 1px; background: Theme.border; opacity: 0.35; }

            // Vertical lines (time divisions)
            Rectangle { x: parent.width * 0.166; width: 1px; height: 100%; background: Theme.border; opacity: 0.35; }
            Rectangle { x: parent.width * 0.333; width: 1px; height: 100%; background: Theme.border; opacity: 0.35; }
            Rectangle { x: parent.width * 0.500; width: 1px; height: 100%; background: Theme.border; opacity: 0.35; }
            Rectangle { x: parent.width * 0.666; width: 1px; height: 100%; background: Theme.border; opacity: 0.35; }
            Rectangle { x: parent.width * 0.833; width: 1px; height: 100%; background: Theme.border; opacity: 0.35; }
        }

        // Paths keep the chart's exact size; only their viewbox slides as
        // ChartClock.phase advances, so the chart scrolls smoothly between samples
        // without touching layout. The newest point enters from the right edge.
        Rectangle {
            width: 100%;
            height: 100%;

            // Shaded fill under curve
            if root.fill-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                fill: root.fill-color;
                commands: root.fill-path;
            }

            // Main telemetry stroke path (crisp GPU antialiased line)
            if root.stroke-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                stroke: root.stroke-color;
                stroke-width: 2px;
                commands: root.stroke-path;
            }

            // Temperature stroke path (e.g. orange #f5a623 for CPU overview)
            if root.has-temp && root.temp-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                stroke: root.temp-color;
                stroke-width: 1.5px;
                commands: root.temp-path;
            }

            // Kernel stroke path (e.g. CPU kernel time)
            if root.has-kernel && root.kernel-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                stroke: root.kernel-color;
                stroke-width: 1.5px;
                commands: root.kernel-path;
            }
        }

        // Committed history selection band with dashed edges
        if root.sel-active : Rectangle {
            property <length> xa: max(0px, root.data-to-x(root.sel-a));
            property <length> xb: min(root.width, root.data-to-x(root.sel-b));
            x: self.xa;
            width: max(0px, self.xb - self.xa);
            height: 100%;
            background: #36e8ff1f;
        }
        if root.sel-active : DashedVLine {
            x: root.data-to-x(root.sel-a);
            visible: self.x >= 0px && self.x <= root.width;
        }
        if root.sel-active : DashedVLine {
            x: root.data-to-x(root.sel-b);
            visible: self.x >= 0px && self.x <= root.width;
        }

        // Live drag preview while selecting
        if sel-ta.pressed && abs(sel-ta.mouse-x - sel-ta.pressed-x) >= 4px : Rectangle {
            property <length> xa: clamp(min(sel-ta.pressed-x, sel-ta.mouse-x), 0px, root.width);
            property <length> xb: clamp(max(sel-ta.pressed-x, sel-ta.mouse-x), 0px, root.width);
            x: self.xa;
            width: self.xb - self.xa;
            height: 100%;
            background: #36e8ff2a;
            border-width: 1px;
            border-color: #36e8ff99;
        }

        // Right axis labels
        if root.right-max-label != "" : Text {
            x: root.width - self.width - 6px;
            y: 28px;
            text: root.right-max-label;
            color: root.temp-color;
            font-family: Theme.font-family;
            font-size: 10px;
        }
        if root.right-mid-label != "" : Text {
            x: root.width - self.width - 6px;
            y: root.height / 2 - self.height / 2;
            text: root.right-mid-label;
            color: root.temp-color;
            font-family: Theme.font-family;
            font-size: 10px;
        }
        if root.right-min-label != "" : Text {
            x: root.width - self.width - 6px;
            y: root.height - self.height - 24px;
            text: root.right-min-label;
            color: root.temp-color;
            font-family: Theme.font-family;
            font-size: 10px;
        }

        // Header and Footer Overlay Labels
        // Top-Left: Title + Max Label
        Rectangle {
            x: 6px;
            y: 5px;
            width: title-hl.preferred-width;
            height: 18px;
            border-radius: 3px;
            background: #00000088;
            title-hl := HorizontalLayout {
                padding-left: 5px;
                padding-right: 5px;
                spacing: 6px;
                Text {
                    text: root.title-label;
                    color: Theme.text-main;
                    font-family: Theme.font-ui;
                    font-size: 11px;
                    font-weight: 600;
                    vertical-alignment: center;
                }
                Text {
                    text: "· max " + root.max-label;
                    color: Theme.text-dim;
                    font-family: Theme.font-family;
                    font-size: 10px;
                    vertical-alignment: center;
                }
            }
        }

        // Top-Right: Current live value badge
        if root.current-val-label != "" : Rectangle {
            x: parent.width - self.width - 6px;
            y: 4px;
            width: badge-hl.preferred-width;
            height: 20px;
            border-radius: 4px;
            background: #000000bb;
            border-width: 1px;
            border-color: root.stroke-color;
            badge-hl := HorizontalLayout {
                padding-left: 7px;
                padding-right: 7px;
                spacing: 5px;
                alignment: center;
                Rectangle {
                    width: 6px;
                    height: 6px;
                    y: (parent.height - self.height) / 2;
                    border-radius: 3px;
                    background: root.stroke-color;
                }
                Text {
                    text: root.current-val-label;
                    color: root.stroke-color;
                    font-family: Theme.font-family;
                    font-size: 11px;
                    font-weight: 700;
                    vertical-alignment: center;
                }
            }
        }

        // Bottom-Left: Min Label
        Rectangle {
            x: 6px;
            y: parent.height - self.height - 4px;
            width: min-txt.preferred-width + 8px;
            height: 15px;
            border-radius: 3px;
            background: #00000088;
            min-txt := Text {
                text: root.min-label;
                color: Theme.text-dim;
                font-family: Theme.font-family;
                font-size: 10px;
                vertical-alignment: center;
                horizontal-alignment: center;
            }
        }

        // Bottom-Right: Time Scale
        Rectangle {
            x: parent.width - self.width - 6px;
            y: parent.height - self.height - 4px;
            width: time-txt.preferred-width + 8px;
            height: 15px;
            border-radius: 3px;
            background: #00000088;
            time-txt := Text {
                text: root.time-label;
                color: Theme.text-dim;
                font-family: Theme.font-ui;
                font-size: 10px;
                vertical-alignment: center;
                horizontal-alignment: center;
            }
        }

        sel-ta := TouchArea {
            enabled: root.selectable;
            mouse-cursor: root.selectable ? MouseCursor.crosshair : MouseCursor.default;
            pointer-event(e) => {
                if (e.kind == PointerEventKind.up && e.button == PointerEventButton.left) {
                    if (abs(self.mouse-x - self.pressed-x) < 4px) {
                        root.range-cleared();
                    } else {
                        root.range-selected(
                            clamp(min(self.pressed-x, self.mouse-x) / root.width, 0, 1),
                            clamp(max(self.pressed-x, self.mouse-x) / root.width, 0, 1));
                    }
                }
            }
        }
    }

    component CoreMiniChart inherits Rectangle {
        // Number of points in the series ring (one is kept off-screen for scrolling).
        in property <int> points: 60;
        // Visible window covers points-2 intervals; it slides by one interval per sample.
        property <float> vb-interval: 1000 / max(1, root.points - 1);
        property <float> vb-x: ChartClock.phase * root.vb-interval;
        property <float> vb-w: 1000 - root.vb-interval;
        in property <string> label: "CPU 0";
        in property <string> stroke-path: "";
        in property <string> fill-path: "";
        in property <string> kernel-path: "";
        in property <string> val-str: "0%";

        background: Theme.ui-style == 1 ? #00000030 : (Theme.ui-style == 2 ? #00000020 : #060a0f);
        border-width: 1px;
        border-color: Theme.border;
        border-radius: Theme.card-radius;
        clip: true;

        // Subtle 1px vector grid
        Rectangle { y: parent.height * 0.33; width: 100%; height: 1px; background: Theme.border; opacity: 0.25; }
        Rectangle { y: parent.height * 0.66; width: 100%; height: 1px; background: Theme.border; opacity: 0.25; }
        Rectangle { x: parent.width * 0.50; width: 1px; height: 100%; background: Theme.border; opacity: 0.25; }

        // Paths keep the chart's exact size; only their viewbox slides as
        // ChartClock.phase advances, so the chart scrolls smoothly between samples
        // without touching layout. The newest point enters from the right edge.
        Rectangle {
            width: 100%;
            height: 100%;

            // Area fill under curve
            if root.fill-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                fill: #36c5ff18;
                commands: root.fill-path;
            }

            // Kernel stroke
            if root.kernel-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                stroke: #ff5566;
                stroke-width: 1px;
                commands: root.kernel-path;
            }

            // Main stroke
            if root.stroke-path != "" : Path {
                width: 100%;
                height: 100%;
                viewbox-x: root.vb-x;
                viewbox-y: 0;
                viewbox-width: root.vb-w;
                viewbox-height: 1000;
                fit: ImageFit.fill;
                stroke: #36c5ff;
                stroke-width: 1.5px;
                commands: root.stroke-path;
            }
        }

        // Core label in clear typography
        Text {
            x: 6px;
            y: 4px;
            text: root.label;
            color: Theme.text-dim;
            font-family: Theme.font-ui;
            font-size: 10px;
            font-weight: 600;
        }

        // Current utilization % in top right
        Text {
            x: parent.width - self.width - 6px;
            y: 4px;
            text: root.val-str;
            color: #36c5ff;
            font-family: Theme.font-family;
            font-size: 10px;
            font-weight: 700;
        }
    }

    export component MainWindow inherits Window {
        title: "PulseDeck";
        preferred-width: 1480px;
        preferred-height: 920px;
        min-width: 1100px;
        min-height: 720px;
        background: Theme.bg-main;
        default-font-family: Theme.font-family;
        default-font-size: Theme.font-size-base;

        // Navigation & UI state
        // 0=Summary, 1=Performance, 2=Processes, 3=System Info, 4=Startup apps, 5=Users, 6=Services,
        // 7=Power & Freq, 8=Flight Recorder, 9=Connections, 10=Installed Apps, 11=Drivers, 12=Disk Space, 13=Benchmarks,
        // 14=Settings, 15=Colors
        in-out property <int> active-tab: 0;
        in-out property <string> page-title: "Summary";
        in-out property <int> perf-selected-idx: 0; // 0=CPU, 1=Memory, 2=Disk, 3=Network, 4=GPU, 5=Energy, 6=Thermals
        in-out property <bool> perf-cpu-expanded: false;
        in-out property <int> open-menu: 0; // 0=none, 1=File, 2=Edit, 3=View, 4=Performance, 5=Help
        in-out property <bool> show-sidebar: true;

        // Bottom status bar properties
        in-out property <string> status-left-text: "Native providers healthy · 0 processes";
        in-out property <string> status-ip-text: "IP 127.0.0.1";
        in-out property <string> status-gen-text: "Generation 1";
        in-out property <string> status-timer-text: "00:00:00.00";
        in-out property <string> status-message: "";

        // Sidebar Logo image
        in property <image> sidebar-logo-img;

        // Summary Page properties
        in property <string> sum-vfd-clock: "4.0GHZ";
        in property <string> sum-vfd-cpu-pct: "16%";
        in property <image> sum-vertical-bars-img;
        in property <string> sum-bar-cpu-val: "16%";
        in property <string> sum-bar-clock-val: "4.0G";
        in property <string> sum-bar-temp-val: "64°";
        in property <string> sum-bar-gpu-val: "37%";

        in property <image> sum-cpu-overview-graph;
        in property <string> sum-cpu-logical-str: "24 logical processors";
        in property <string> sum-cpu-speed-str: "Speed 4.00 GHz";
        in property <string> sum-top-procs-banner: "TOP 18 CPU PROCESSES";
        in property <[ProcRow]> top-processes;
        in property <string> sum-top-procs-banner-right;
        // CPU history (Summary): rolling window + optional selected time range
        in property <int> sum-cpu-points: 120;
        in property <string> sum-cpu-window-label: "10 min";
        in property <string> sum-cpu-hint: "Drag on the chart to inspect a past time range";
        in property <bool> sum-sel-active: false;
        in property <float> sum-sel-a: 0;
        in property <float> sum-sel-b: 0;
        in property <string> sum-sel-summary;
        callback history-range-selected(float, float);
        callback set-history-window(int);
        callback history-range-cleared();

        in property <image> sum-mem-vert-bar-img;
        in property <string> sum-mem-pct-val: "27%";
        in property <string> sum-mem-header-right: "8.5 GB / 31.3 GB";
        in property <image> sum-mem-graph;
        in property <string> sum-mem-avail-str: "Available 22.8 GB";
        in property <string> sum-mem-cached-str: "Cached 22.2 GB";
        in property <string> sum-mem-swap-str: "Swap 2.1 MB";

        // Summary Vector Graphics properties
        in property <string> sum-cpu-vector-stroke: "";
        in property <string> sum-cpu-vector-fill: "";
        in property <string> sum-cpu-vector-temp: "";
        in property <string> sum-cpu-vector-kernel: "";
        in property <string> sum-mem-vector-stroke: "";
        in property <string> sum-mem-vector-fill: "";

        // Summary Bottom 4 Cards
        in property <string> sum-card1-right: "LOW";
        in property <color> sum-card1-color: #22e58b;
        in property <string> sum-card1-sub: "Load demand 18.0% · Memory stalls 0.0%";
        in property <image> sum-card1-bar;

        in property <string> sum-card2-title: "enp5s0";
        in property <string> sum-card2-right: "0.0 KB/sec";
        in property <string> sum-card2-sub-left: "1 GbE · 0.0%";
        in property <string> sum-card2-sub-right: "Scale 33.3 MB/sec";
        in property <image> sum-card2-bar;

        in property <string> sum-card3-right: "0.0%";
        in property <string> sum-card3-sub-left: "R 0 B/sec · W 0 B/sec";
        in property <image> sum-card3-bar;

        in property <string> sum-card4-title: "GPU";
        in property <string> sum-card4-right: "42 °C";
        in property <string> sum-card4-sub: "GPU Power: 45.0 W";
        in property <image> sum-card4-bar;

        // Performance Page properties
        in property <[PerfNavItem]> perf-nav-items;
        in property <[CorePlotData]> perf-core-items;
        in property <int> perf-core-cols: 8;
        in property <int> perf-core-rows: 2;
        in property <string> perf-header-title: "CPU";
        in property <string> perf-header-pct: "19.4%";
        in property <string> perf-header-sub: "24 logical processors";
        in property <image> perf-header-led-bar;
        in property <image> perf-main-graph-img;
        in property <image> perf-secondary-graph-img;
        in property <string> perf-main-graph-label: "% Utilization";
        in property <string> perf-sec-graph-label: "GPU memory in use";
        in property <string> perf-sec-graph-right: "";
        in property <bool> perf-has-secondary-graph: false;

        // Performance Native Vector Graphics properties
        in property <string> perf-vector-stroke: "";
        in property <string> perf-vector-fill: "";
        in property <string> perf-vector-kernel: "";
        in property <color> perf-vector-color: #36c5ff;
        in property <color> perf-vector-fill-color: #36c5ff22;
        in property <bool> perf-vector-has-kernel: false;
        in property <string> perf-vector-current-val: "";
        in property <string> perf-vector-max-label: "100%";
        in property <string> perf-vector-min-label: "0%";

        // Performance Secondary Vector Graphics properties
        in property <string> perf-sec-vector-stroke: "";
        in property <string> perf-sec-vector-fill: "";
        in property <color> perf-sec-vector-color: #b84dff;
        in property <color> perf-sec-vector-fill-color: #b84dff22;
        in property <string> perf-sec-vector-current-val: "";
        in property <string> perf-sec-vector-max-label: "100%";
        in property <string> perf-sec-vector-min-label: "0%";

        // Performance Bottom Readouts (4 primary metric blocks + 8 detail kv pairs)
        in property <string> perf-m1-label: "Utilization";
        in property <string> perf-m1-val: "19.4%";
        in property <string> perf-m2-label: "Speed";
        in property <string> perf-m2-val: "3.78 GHz";
        in property <string> perf-m3-label: "Processes";
        in property <string> perf-m3-val: "594";
        in property <string> perf-m4-label: "Threads";
        in property <string> perf-m4-val: "2122";
        in property <string> perf-m5-label: "Up time";
        in property <string> perf-m5-val: "0:02:27:54";
        in property <string> perf-m6-label: "";
        in property <string> perf-m6-val: "";
        in property <string> perf-m7-label: "";
        in property <string> perf-m7-val: "";

        in property <string> perf-kv1-k: "Base speed:";
        in property <string> perf-kv1-v: "4.67 GHz";
        in property <string> perf-kv2-k: "Physical cores:";
        in property <string> perf-kv2-v: "12";
        in property <string> perf-kv3-k: "Virtualization:";
        in property <string> perf-kv3-v: "Hardware virtualization supported";
        in property <string> perf-kv4-k: "Frequency driver:";
        in property <string> perf-kv4-v: "amd-pstate-epp";
        in property <string> perf-kv5-k: "Sockets:";
        in property <string> perf-kv5-v: "1";
        in property <string> perf-kv6-k: "Logical processors:";
        in property <string> perf-kv6-v: "24";
        in property <string> perf-kv7-k: "Virtual machine:";
        in property <string> perf-kv7-v: "No";
        in property <string> perf-kv8-k: "Governor:";
        in property <string> perf-kv8-v: "powersave";

        // Other Tabs Models
        in property <[ProcRow]> processes;
        in property <[ConnRow]> connections;
        in property <[SensorRow]> sensors;
        in property <[DiskUsageRow]> disk-dirs;
        in property <[DiskUsageRow]> disk-categories;
        in property <[DiskUsageRow]> disk-largest-files;
        in property <[DriverRow]> drivers;
        in property <[CatalogRow]> catalog-rows;
        in property <image> flight-graph;

        // Filter & control properties
        in-out property <string> proc-filter: "";
        in-out property <bool> proc-tree-view: false;
        in-out property <bool> proc-hide-kthreads: true;
        in-out property <string> inspector-title: "Select a process and click 'Inspect' to view open FDs, environment, and memory maps.";
        in-out property <string> inspector-body: "";

        in-out property <string> conn-filter: "";
        in-out property <bool> conn-public-only: false;

        in-out property <string> disk-scan-path: "";
        in-out property <string> disk-scan-summary: "";

        in-out property <string> drv-filter: "";
        in-out property <string> drv-summary: "";

        in-out property <int> flight-scrub-index: 0;
        in-out property <int> flight-frame-count: 0;
        in-out property <string> flight-frame-summary: "";

        in-out property <string> bench-composite: "--";
        in-out property <string> bench-sc: "--";
        in-out property <string> bench-sc-sub: "Not run yet";
        in-out property <string> bench-mc: "--";
        in-out property <string> bench-mc-sub: "Not run yet";
        in-out property <string> bench-mem: "--";
        in-out property <string> bench-mem-sub: "Not run yet";
        in-out property <string> bench-disk: "--";
        in-out property <string> bench-disk-sub: "Not run yet";

        in-out property <string> catalog-filter: "";
        in-out property <string> sysinfo-text: "";

        // Customisation properties
        in-out property <string> current-theme-name: "VFD Phosphor Green";
        in-out property <string> custom-primary-hex: "#22E58B";
        in-out property <string> custom-secondary-hex: "#19B5FE";
        in-out property <string> custom-bg-hex: "#090B0E";
        in-out property <string> custom-card-hex: "#0B0E12";
        in-out property <string> custom-text-hex: "#E6EDF3";
        in-out property <string> custom-font-family: "DejaVu Sans Mono";
        in-out property <int> custom-font-size: 12;
        in-out property <int> custom-refresh-ms: 1000;
        in-out property <bool> custom-compact-rows: false;
        in-out property <bool> custom-show-grid: true;
        in-out property <int> chart-fps: 60; // 0 = smooth scrolling off

        // Context menus state
        in-out property <bool> proc-context-open: false;
        in-out property <int> proc-context-pid: 0;
        in-out property <string> proc-context-name: "";
        in-out property <string> proc-context-cmdline: "";
        in-out property <length> proc-context-x: 200px;
        in-out property <length> proc-context-y: 200px;

        in-out property <bool> cpu-context-open: false;
        in-out property <length> cpu-context-x: 200px;
        in-out property <length> cpu-context-y: 200px;

        // Root / Elevation status
        in-out property <bool> is-root-user: false;

        // Callbacks
        callback select-tab(int, string);
        callback select-perf-item(int);
        callback refresh-now();
        callback refresh-filters();
        callback inspect-process(int);
        callback signal-process(int, string);
        callback scan-disk-space(string);
        callback scrub-flight(int);
        callback export-flight-trace();
        callback run-benchmarks();
        callback export-sysinfo();
        callback apply-theme-preset(string);
        callback apply-custom-colors();
        callback select-font-family(string);
        callback adjust-font-size(int);
        callback set-refresh-rate(int);
        callback toggle-compact-rows();
        callback toggle-graph-grid();
        callback cycle-chart-fps();
        callback save-settings();
        callback reset-settings();
        callback quit-app();
        callback toggle-flight-recording(bool);
        callback copy-to-clipboard(string);
        callback relaunch-as-root();
        callback toggle-windows-default-taskmgr(bool);

        VerticalLayout {
            // 1. Top Menu Strip ("File  Edit  View  Performance  Help")
            Rectangle {
                height: 28px;
                background: Theme.bg-header;
                border-width: 1px;
                border-color: Theme.border;
                HorizontalLayout {
                    padding-left: 6px;
                    padding-right: 12px;
                    padding-top: 2px;
                    padding-bottom: 2px;
                    alignment: space-between;

                    HorizontalLayout {
                        spacing: 4px;
                        alignment: start;
                        MenuBarButton {
                            label: "File";
                            active: root.open-menu == 1;
                            clicked => { root.open-menu = root.open-menu == 1 ? 0 : 1; }
                            hovered => { if (root.open-menu != 0) { root.open-menu = 1; } }
                        }
                        MenuBarButton {
                            label: "Edit";
                            active: root.open-menu == 2;
                            clicked => { root.open-menu = root.open-menu == 2 ? 0 : 2; }
                            hovered => { if (root.open-menu != 0) { root.open-menu = 2; } }
                        }
                        MenuBarButton {
                            label: "View";
                            active: root.open-menu == 3;
                            clicked => { root.open-menu = root.open-menu == 3 ? 0 : 3; }
                            hovered => { if (root.open-menu != 0) { root.open-menu = 3; } }
                        }
                        MenuBarButton {
                            label: "Performance";
                            active: root.open-menu == 4;
                            clicked => { root.open-menu = root.open-menu == 4 ? 0 : 4; }
                            hovered => { if (root.open-menu != 0) { root.open-menu = 4; } }
                        }
                        MenuBarButton {
                            label: "Help";
                            active: root.open-menu == 5;
                            clicked => { root.open-menu = root.open-menu == 5 ? 0 : 5; }
                            hovered => { if (root.open-menu != 0) { root.open-menu = 5; } }
                        }
                    }

                    if root.is-root-user : Rectangle {
                        height: 20px;
                        background: #3d1414;
                        border-width: 1px;
                        border-color: Theme.accent-danger;
                        border-radius: 4px;
                        HorizontalLayout {
                            padding-left: 8px;
                            padding-right: 8px;
                            Text {
                                text: "🛡️ ROOT ELEVATED";
                                color: #ff6b6b;
                                font-family: Theme.font-ui;
                                font-size: 10px;
                                font-weight: 700;
                                vertical-alignment: center;
                            }
                        }
                    }
                }
            }

            // 2. Main Body (Left Navigation Sidebar + Active Page Workspace)
            HorizontalLayout {
                vertical-stretch: 1;

                // Left Navigation Sidebar
                if root.show-sidebar : Rectangle {
                    width: 196px;
                    background: Theme.bg-panel;
                    border-width: 1px;
                    border-color: Theme.border;

                    VerticalLayout {
                        padding: 8px;
                        spacing: 2px;

                        SidebarItem { icon: "⌂"; label: "Summary"; active: root.active-tab == 0; clicked => { root.select-tab(0, "Summary"); } }
                        SidebarItem { icon: "≋"; label: "Performance"; active: root.active-tab == 1; clicked => { root.select-tab(1, "Performance"); } }
                        SidebarItem { icon: "▤"; label: "Processes"; active: root.active-tab == 2; clicked => { root.select-tab(2, "Processes"); } }
                        SidebarItem { icon: "⊙"; label: "System Info"; active: root.active-tab == 3; clicked => { root.select-tab(3, "System Info"); } }
                        SidebarItem { icon: "▷"; label: "Startup apps"; active: root.active-tab == 4; clicked => { root.select-tab(4, "Startup apps"); } }
                        SidebarItem { icon: "⚇"; label: "Users"; active: root.active-tab == 5; clicked => { root.select-tab(5, "Users"); } }
                        SidebarItem { icon: "⚒"; label: "Services"; active: root.active-tab == 6; clicked => { root.select-tab(6, "Services"); } }

                        // Clean Section Separator
                        Rectangle {
                            height: 14px;
                            VerticalLayout {
                                alignment: center;
                                Rectangle { height: 1px; background: Theme.border; }
                            }
                        }

                        SidebarItem { icon: "⚡"; label: "Power & Freq"; active: root.active-tab == 7; clicked => { root.select-tab(7, "Power & Freq"); } }
                        SidebarItem { icon: "◉"; label: "Flight Recorder"; active: root.active-tab == 8; clicked => { root.select-tab(8, "Flight Recorder"); } }
                        SidebarItem { icon: "⇄"; label: "Connections"; active: root.active-tab == 9; clicked => { root.select-tab(9, "Connections"); } }
                        SidebarItem { icon: "▦"; label: "Installed Apps"; active: root.active-tab == 10; clicked => { root.select-tab(10, "Installed Apps"); } }
                        SidebarItem { icon: "⚙"; label: "Drivers"; active: root.active-tab == 11; clicked => { root.select-tab(11, "Drivers"); } }
                        SidebarItem { icon: "◔"; label: "Disk Space"; active: root.active-tab == 12; clicked => { root.select-tab(12, "Disk Space"); } }
                        SidebarItem { icon: "◷"; label: "Benchmarks"; active: root.active-tab == 13; clicked => { root.select-tab(13, "Benchmarks"); } }

                        Rectangle { vertical-stretch: 1; }

                        // Bottom Glowing ECG Pulse Logo
                        Rectangle {
                            height: 54px;
                            Image {
                                width: 100%;
                                height: 100%;
                                source: root.sidebar-logo-img;
                                image-fit: contain;
                            }
                        }

                        // Bottom Settings & Colors Buttons
                        HorizontalLayout {
                            spacing: 6px;
                            height: 28px;
                            Rectangle {
                                horizontal-stretch: 1;
                                border-radius: 4px;
                                background: root.active-tab == 14 ? Theme.nav-selected : (set-ta.has-hover ? Theme.menu-hover : transparent);
                                border-width: root.active-tab == 14 ? 1px : 0px;
                                border-color: Theme.nav-border;
                                HorizontalLayout {
                                    alignment: center;
                                    spacing: 4px;
                                    Text { text: "⚙"; color: root.active-tab == 14 ? Theme.accent-primary : Theme.text-dim; font-family: "DejaVu Sans Mono"; font-size: 13px; vertical-alignment: center; }
                                    Text { text: "Settings"; color: root.active-tab == 14 ? #ffffff : Theme.text-main; font-family: Theme.font-ui; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                }
                                set-ta := TouchArea { mouse-cursor: pointer; clicked => { root.select-tab(14, "Settings"); } }
                            }
                            Rectangle {
                                horizontal-stretch: 1;
                                border-radius: 4px;
                                background: root.active-tab == 15 ? Theme.nav-selected : (col-ta.has-hover ? Theme.menu-hover : transparent);
                                border-width: root.active-tab == 15 ? 1px : 0px;
                                border-color: Theme.nav-border;
                                HorizontalLayout {
                                    alignment: center;
                                    spacing: 4px;
                                    Text { text: "✦"; color: root.active-tab == 15 ? Theme.accent-primary : Theme.text-dim; font-family: "DejaVu Sans Mono"; font-size: 13px; vertical-alignment: center; }
                                    Text { text: "Colors"; color: root.active-tab == 15 ? #ffffff : Theme.text-main; font-family: Theme.font-ui; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                }
                                col-ta := TouchArea { mouse-cursor: pointer; clicked => { root.select-tab(15, "Colors & Theme"); } }
                            }
                        }
                    }
                }

                // Right Main Content Area
                VerticalLayout {
                    horizontal-stretch: 1;
                    padding: 14px;
                    spacing: 10px;

                    // Page Top Header ("≡ Summary" on left, "↻ Refresh" on right)
                    HorizontalLayout {
                        height: 34px;
                        alignment: space-between;
                        HorizontalLayout {
                            spacing: 10px;
                            alignment: start;
                            Rectangle {
                                width: 28px;
                                height: 28px;
                                border-radius: 4px;
                                background: ham-ta.has-hover ? Theme.menu-hover : transparent;
                                Text {
                                    text: "≡";
                                    color: Theme.text-dim;
                                    font-size: 20px;
                                    horizontal-alignment: center;
                                    vertical-alignment: center;
                                }
                                ham-ta := TouchArea {
                                    mouse-cursor: pointer;
                                    clicked => { root.show-sidebar = !root.show-sidebar; }
                                }
                            }
                            Text {
                                text: root.page-title;
                                color: Theme.text-main;
                                font-size: Theme.font-size-xl;
                                font-weight: 700;
                                vertical-alignment: center;
                            }
                        }
                        ActionBtn {
                            label: "↻  Refresh";
                            clicked => { root.refresh-now(); }
                        }
                    }

                    // ============================================================
                    // TAB 0: SUMMARY (Exact Match to Screenshot 1)
                    // ============================================================
                    if root.active-tab == 0 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        // Top Row: 3 Cards (SYS 4-Bar Cluster | CPU OVERVIEW | TOP 18 CPU PROCESSES)
                        HorizontalLayout {
                            vertical-stretch: 1;
                            spacing: 10px;

                            // Card 1: SYS | 4.0GHZ Vertical Segmented LED Bars
                            Rectangle {
                                width: 215px;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;

                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 6px;

                                    VfdBanner {
                                        left-text: "SYS";
                                        right-text: root.sum-vfd-clock;
                                    }

                                    HorizontalLayout {
                                        height: 16px;
                                        Text { horizontal-stretch: 1; text: "CPU"; color: #22e58b; font-size: Theme.font-size-sm; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "Clock"; color: #ff4757; font-size: Theme.font-size-sm; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "Temp"; color: #f5a623; font-size: Theme.font-size-sm; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "GPU"; color: #19b5fe; font-size: Theme.font-size-sm; horizontal-alignment: center; }
                                    }

                                    Image {
                                        vertical-stretch: 1;
                                        width: 100%;
                                        source: root.sum-vertical-bars-img;
                                        image-fit: fill;
                                    }

                                    HorizontalLayout {
                                        height: 18px;
                                        Text { horizontal-stretch: 1; text: root.sum-bar-cpu-val; color: #22e58b; font-size: Theme.font-size-sm; font-weight: 700; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: root.sum-bar-clock-val; color: #ff4757; font-size: Theme.font-size-sm; font-weight: 700; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: root.sum-bar-temp-val; color: #f5a623; font-size: Theme.font-size-sm; font-weight: 700; horizontal-alignment: center; }
                                        Text { horizontal-stretch: 1; text: root.sum-bar-gpu-val; color: #19b5fe; font-size: Theme.font-size-sm; font-weight: 700; horizontal-alignment: center; }
                                    }
                                }
                            }

                            // Card 2: CPU OVERVIEW | 16%
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;

                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 6px;

                                    VfdBanner {
                                        left-text: "CPU HISTORY";
                                        right-text: root.sum-vfd-cpu-pct;
                                    }

                                    HorizontalLayout {
                                        height: 18px;
                                        spacing: 14px;
                                        Text { text: "〰 Utilization"; color: #22e58b; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        Text { text: "🌡 Temperature"; color: #f5a623; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        Text { text: "⚙ Kernel"; color: #b84dff; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        Rectangle { horizontal-stretch: 1; }
                                        // History zoom: visible window over the 10-minute buffer
                                        HorizontalLayout {
                                            spacing: 4px;
                                            for z in [{ n: 120, t: "2m" }, { n: 300, t: "5m" }, { n: 600, t: "10m" }] : Rectangle {
                                                width: 34px;
                                                border-radius: 3px;
                                                border-width: 1px;
                                                border-color: root.sum-cpu-points == z.n ? #36e8ff : Theme.border;
                                                background: root.sum-cpu-points == z.n ? #36e8ff22 : (zoom-ta.has-hover ? Theme.menu-hover : transparent);
                                                Text {
                                                    text: z.t;
                                                    color: root.sum-cpu-points == z.n ? #36e8ff : Theme.text-dim;
                                                    font-size: 10px;
                                                    horizontal-alignment: center;
                                                    vertical-alignment: center;
                                                }
                                                zoom-ta := TouchArea {
                                                    mouse-cursor: pointer;
                                                    clicked => { root.set-history-window(z.n); }
                                                }
                                            }
                                        }
                                    }

                                    VectorTelemetryChart {
                                        vertical-stretch: 1;
                                        stroke-path: root.sum-cpu-vector-stroke;
                                        fill-path: root.sum-cpu-vector-fill;
                                        temp-path: root.sum-cpu-vector-temp;
                                        has-temp: true;
                                        kernel-path: root.sum-cpu-vector-kernel;
                                        has-kernel: true;
                                        stroke-color: #22e58b;
                                        fill-color: #22e58b22;
                                        temp-color: #f5a623;
                                        kernel-color: #b84dff;
                                        show-grid: true;
                                        title-label: "CPU History";
                                        current-val-label: root.sum-vfd-cpu-pct;
                                        max-label: "100%";
                                        min-label: "0%";
                                        time-label: root.sum-cpu-window-label;
                                        points: root.sum-cpu-points;
                                        right-max-label: "110°";
                                        right-mid-label: "55°";
                                        right-min-label: "0°";
                                        selectable: true;
                                        sel-active: root.sum-sel-active;
                                        sel-a: root.sum-sel-a;
                                        sel-b: root.sum-sel-b;
                                        range-selected(a, b) => { root.history-range-selected(a, b); }
                                        range-cleared => { root.history-range-cleared(); }
                                    }

                                    HorizontalLayout {
                                        height: 28px;
                                        alignment: space-between;
                                        VerticalLayout {
                                            spacing: 1px;
                                            Text { text: root.sum-cpu-hint; color: root.sum-sel-active ? #36e8ff : Theme.text-dim; font-size: 10px; }
                                            Text { text: root.sum-cpu-logical-str; color: Theme.text-dim; font-size: 10px; }
                                        }
                                        VerticalLayout {
                                            alignment: end;
                                            Text { text: root.sum-cpu-speed-str; color: Theme.text-dim; font-size: 10px; horizontal-alignment: right; }
                                        }
                                    }
                                }
                            }

                            // Card 3: TOP 18 CPU PROCESSES
                            Rectangle {
                                width: 395px;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;

                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 4px;

                                    VfdBanner {
                                        left-text: root.sum-top-procs-banner;
                                        right-text: root.sum-top-procs-banner-right;
                                    }

                                    if root.sum-sel-active : Rectangle {
                                        height: 20px;
                                        border-radius: 3px;
                                        background: live-ta.has-hover ? #36e8ff30 : #36e8ff18;
                                        border-width: 1px;
                                        border-color: #36e8ff66;
                                        Text {
                                            text: "◀ " + root.sum-sel-summary + "  ·  back to live";
                                            color: #36e8ff;
                                            font-size: 10px;
                                            horizontal-alignment: center;
                                            vertical-alignment: center;
                                        }
                                        live-ta := TouchArea {
                                            mouse-cursor: pointer;
                                            clicked => { root.history-range-cleared(); }
                                        }
                                    }

                                    HorizontalLayout {
                                        height: 18px;
                                        padding-left: 4px;
                                        padding-right: 4px;
                                        spacing: 6px;
                                        Text { width: 56px; text: "PID"; color: Theme.text-dim; font-size: 10px; vertical-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "Name"; color: Theme.text-dim; font-size: 10px; vertical-alignment: center; }
                                        Text { width: 44px; text: root.sum-sel-active ? "Avg CPU" : "CPU"; color: Theme.text-dim; font-size: 10px; horizontal-alignment: right; vertical-alignment: center; }
                                        Text { width: 40px; text: root.sum-sel-active ? "Peak" : "GPU"; color: Theme.text-dim; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                                        Text { width: 66px; text: root.sum-sel-active ? "Avg Mem" : "Memory"; color: Theme.text-dim; font-size: 10px; horizontal-alignment: right; vertical-alignment: center; }
                                    }

                                    ScrollView {
                                        vertical-stretch: 1;
                                        VerticalLayout {
                                            spacing: 1px;
                                            for p in root.top-processes : Rectangle {
                                                height: 18px;
                                                border-radius: 2px;
                                                background: top-ta.has-hover ? Theme.menu-hover : (p.highlight_kind == 2 ? #3d1c22 : (p.highlight_kind == 1 ? #163624 : transparent));
                                                border-width: top-ta.has-hover ? 1px : 0px;
                                                border-color: Theme.nav-border;
                                                HorizontalLayout {
                                                    padding-left: 4px;
                                                    padding-right: 4px;
                                                    spacing: 6px;
                                                    Text { width: 56px; text: p.pid; color: Theme.text-dim; font-size: 11px; vertical-alignment: center; }
                                                    Text { horizontal-stretch: 1; text: p.name; color: Theme.text-main; font-size: 11px; vertical-alignment: center; overflow: elide; }
                                                    Text { width: 44px; text: p.cpu_str; color: Theme.accent-primary; font-size: 11px; vertical-alignment: center; horizontal-alignment: right; }
                                                    Text { width: 40px; text: p.gpu_str; color: Theme.text-dim; font-size: 11px; vertical-alignment: center; horizontal-alignment: center; }
                                                    Text { width: 66px; text: p.rss_str; color: Theme.text-main; font-size: 11px; vertical-alignment: center; horizontal-alignment: right; }
                                                }
                                                top-ta := TouchArea {
                                                    mouse-cursor: pointer;
                                                    clicked => {
                                                        root.inspect-process(p.pid);
                                                        root.select-tab(2, "Processes");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Middle Row: Full-Width Memory Utilization Card
                        Rectangle {
                            height: 175px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 8px;

                            HorizontalLayout {
                                padding: 10px;
                                spacing: 12px;

                                // Left Vertical Purple Segmented LED Bar
                                VerticalLayout {
                                    width: 34px;
                                    spacing: 4px;
                                    Image {
                                        vertical-stretch: 1;
                                        width: 100%;
                                        source: root.sum-mem-vert-bar-img;
                                        image-fit: fill;
                                    }
                                    Text {
                                        text: root.sum-mem-pct-val;
                                        color: #b84dff;
                                        font-size: 11px;
                                        font-weight: 700;
                                        horizontal-alignment: center;
                                    }
                                }

                                // Right Memory Chart & Labels
                                VerticalLayout {
                                    horizontal-stretch: 1;
                                    spacing: 4px;

                                    HorizontalLayout {
                                        height: 20px;
                                        alignment: space-between;
                                        Text {
                                            text: "Memory Utilization";
                                            color: Theme.text-main;
                                            font-size: Theme.font-size-md;
                                            font-weight: 700;
                                            vertical-alignment: center;
                                        }
                                        Text {
                                            text: root.sum-mem-header-right;
                                            color: #b84dff;
                                            font-size: Theme.font-size-md;
                                            font-weight: 700;
                                            vertical-alignment: center;
                                        }
                                    }

                                    VectorTelemetryChart {
                                        vertical-stretch: 1;
                                        stroke-path: root.sum-mem-vector-stroke;
                                        fill-path: root.sum-mem-vector-fill;
                                        stroke-color: #b84dff;
                                        fill-color: #b84dff22;
                                        show-grid: true;
                                        title-label: "Memory";
                                        current-val-label: root.sum-mem-pct-val;
                                        max-label: "100%";
                                        min-label: "0%";
                                        time-label: "60s";
                                    }

                                    Text { text: "Physical memory in use"; color: Theme.text-dim; font-size: 10px; }

                                    HorizontalLayout {
                                        height: 14px;
                                        alignment: space-between;
                                        Text { text: root.sum-mem-avail-str; color: Theme.text-dim; font-size: 10px; }
                                        Text { text: root.sum-mem-cached-str; color: Theme.text-dim; font-size: 10px; }
                                        Text { text: root.sum-mem-swap-str; color: Theme.text-dim; font-size: 10px; }
                                    }
                                }
                            }
                        }

                        // Bottom Row: 4 Horizontal Segmented LED Cards (System Pressure, Network, Disk, GPU)
                        HorizontalLayout {
                            height: 92px;
                            spacing: 10px;

                            // Bottom Card 1: System Pressure
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;
                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 4px;
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: "System Pressure"; color: Theme.text-main; font-size: Theme.font-size-sm; font-weight: 700; }
                                        Text { text: root.sum-card1-right; color: root.sum-card1-color; font-size: Theme.font-size-sm; font-weight: 700; }
                                    }
                                    Text { text: root.sum-card1-sub; color: Theme.text-dim; font-size: 11px; overflow: elide; }
                                    Image { vertical-stretch: 1; width: 100%; source: root.sum-card1-bar; image-fit: fill; }
                                }
                            }

                            // Bottom Card 2: Network Adapter
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;
                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 4px;
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: root.sum-card2-title; color: Theme.text-main; font-size: Theme.font-size-sm; font-weight: 700; }
                                        Text { text: root.sum-card2-right; color: #19b5fe; font-size: Theme.font-size-sm; font-weight: 700; }
                                    }
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: root.sum-card2-sub-left; color: Theme.text-dim; font-size: 11px; }
                                        Text { text: root.sum-card2-sub-right; color: Theme.text-main; font-size: 11px; }
                                    }
                                    Image { vertical-stretch: 1; width: 100%; source: root.sum-card2-bar; image-fit: fill; }
                                }
                            }

                            // Bottom Card 3: Disk
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;
                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 4px;
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: "Disk"; color: Theme.text-main; font-size: Theme.font-size-sm; font-weight: 700; }
                                        Text { text: root.sum-card3-right; color: #22e58b; font-size: Theme.font-size-sm; font-weight: 700; }
                                    }
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: root.sum-card3-sub-left; color: Theme.text-dim; font-size: 11px; overflow: elide; }
                                        Text { text: "SMART: Healthy"; color: #22e58b; font-size: 11px; }
                                    }
                                    Image { vertical-stretch: 1; width: 100%; source: root.sum-card3-bar; image-fit: fill; }
                                }
                            }

                            // Bottom Card 4: Primary GPU
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 8px;
                                VerticalLayout {
                                    padding: 10px;
                                    spacing: 4px;
                                    HorizontalLayout {
                                        alignment: space-between;
                                        Text { text: root.sum-card4-title; color: Theme.text-main; font-size: Theme.font-size-sm; font-weight: 700; overflow: elide; }
                                        Text { text: root.sum-card4-right; color: #19b5fe; font-size: Theme.font-size-sm; font-weight: 700; }
                                    }
                                    Text { text: root.sum-card4-sub; color: Theme.text-dim; font-size: 11px; }
                                    Image { vertical-stretch: 1; width: 100%; source: root.sum-card4-bar; image-fit: fill; }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 1: PERFORMANCE (Exact Match to Screenshots 2 & 3)
                    // ============================================================
                    if root.active-tab == 1 : HorizontalLayout {
                        vertical-stretch: 1;
                        spacing: 14px;

                        // Left Device / Resource Selector List with Mini Sparklines
                        Rectangle {
                            width: 285px;
                            background: Theme.bg-card;
                            border-width: 2px;
                            border-color: #1976d2;
                            border-radius: 8px;

                            ScrollView {
                                VerticalLayout {
                                    padding: 6px;
                                    spacing: 4px;
                                    for item in root.perf-nav-items : Rectangle {
                                        height: 62px;
                                        border-radius: 6px;
                                        background: root.perf-selected-idx == item.index ? Theme.nav-selected : (pnav-ta.has-hover ? #141b24 : transparent);
                                        border-width: root.perf-selected-idx == item.index ? 1px : 0px;
                                        border-color: Theme.nav-border;

                                        HorizontalLayout {
                                            padding: 8px;
                                            spacing: 10px;
                                            Text {
                                                width: 20px;
                                                text: item.icon;
                                                color: item.icon_color;
                                                font-size: 15px;
                                                vertical-alignment: center;
                                            }
                                            VerticalLayout {
                                                horizontal-stretch: 1;
                                                alignment: center;
                                                spacing: 2px;
                                                Text { text: item.title; color: Theme.text-main; font-size: Theme.font-size-base; font-weight: 700; overflow: elide; }
                                                Text { text: item.sub1; color: Theme.text-dim; font-size: 10px; overflow: elide; }
                                                Text { text: item.sub2; color: Theme.text-dim; font-size: 10px; overflow: elide; }
                                            }
                                            Image {
                                                width: 64px;
                                                height: 42px;
                                                source: item.sparkline;
                                                image-fit: fill;
                                            }
                                        }
                                        pnav-ta := TouchArea {
                                            mouse-cursor: pointer;
                                            clicked => { root.select-perf-item(item.index); }
                                        }
                                    }
                                }
                            }
                        }

                        // Right Detail Pane
                        VerticalLayout {
                            horizontal-stretch: 1;
                            spacing: 8px;

                            // Title ("CPU" or "AMD Radeon RX 9070 XT")
                            Text {
                                text: root.perf-header-title;
                                color: Theme.text-main;
                                font-size: 22px;
                                font-weight: 700;
                            }

                            // Horizontal Glowing Segmented LED Bar + Large Cyan Percentage
                            HorizontalLayout {
                                height: 34px;
                                spacing: 16px;
                                Image {
                                    horizontal-stretch: 1;
                                    height: 32px;
                                    source: root.perf-header-led-bar;
                                    image-fit: fill;
                                }
                                Text {
                                    width: 90px;
                                    text: root.perf-header-pct;
                                    color: #36c5ff;
                                    font-size: 20px;
                                    font-weight: 700;
                                    vertical-alignment: center;
                                    horizontal-alignment: right;
                                }
                            }

                            // Subtitle ("24 logical processors" or "0000:0b:00.0 · amdgpu")
                            Text {
                                text: root.perf-header-sub;
                                color: Theme.text-main;
                                font-size: Theme.font-size-base;
                            }

                            // Sub-header row
                            HorizontalLayout {
                                height: 24px;
                                alignment: space-between;
                                Text {
                                    text: root.perf-selected-idx == 0 ? "% Utilization by logical processor" : root.perf-main-graph-label;
                                    color: Theme.text-main;
                                    font-size: Theme.font-size-base;
                                    vertical-alignment: center;
                                }
                                if root.perf-selected-idx == 0 : ActionBtn {
                                    label: root.perf-cpu-expanded ? "CPUs Expanded  ▾" : "Overall Graph  ▾";
                                    clicked => {
                                        root.perf-cpu-expanded = !root.perf-cpu-expanded;
                                        root.refresh-now();
                                    }
                                }
                            }

                            // Main Chart Area (Per-Core Grid when CPU Expanded, else Large Chart)
                            Rectangle {
                                vertical-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: Theme.card-radius;
                                clip: true;

                                // Interactive Right-Click Context Menu & Double-Click Toggle
                                ta := TouchArea {
                                    mouse-cursor: pointer;
                                    pointer-event(event) => {
                                        if (event.button == PointerEventButton.right && event.kind == PointerEventKind.down) {
                                            root.cpu-context-x = ta.mouse-x + 230px;
                                            root.cpu-context-y = ta.mouse-y + 120px;
                                            root.cpu-context-open = true;
                                        }
                                    }
                                    double-clicked => {
                                        if (root.perf-selected-idx == 0) {
                                            root.perf-cpu-expanded = !root.perf-cpu-expanded;
                                            root.refresh-now();
                                        }
                                    }
                                }

                                if root.perf-selected-idx == 0 && root.perf-cpu-expanded : Rectangle {
                                    width: 100%;
                                    height: 100%;
                                    clip: true;

                                    for core[idx] in root.perf-core-items : CoreMiniChart {
                                        x: Math.mod(idx, root.perf-core-cols) * (parent.width / root.perf-core-cols) + 2px;
                                        y: Math.floor(idx / root.perf-core-cols) * (parent.height / root.perf-core-rows) + 2px;
                                        width: (parent.width / root.perf-core-cols) - 4px;
                                        height: (parent.height / root.perf-core-rows) - 4px;
                                        label: core.label;
                                        stroke-path: core.stroke-path;
                                        fill-path: core.fill-path;
                                        kernel-path: core.kernel-path;
                                        val-str: core.val-str;
                                    }
                                }

                                if !(root.perf-selected-idx == 0 && root.perf-cpu-expanded) : VerticalLayout {
                                    spacing: 6px;
                                    padding: 4px;
                                    VectorTelemetryChart {
                                        vertical-stretch: root.perf-has-secondary-graph ? 3 : 1;
                                        stroke-path: root.perf-vector-stroke;
                                        fill-path: root.perf-vector-fill;
                                        kernel-path: root.perf-vector-kernel;
                                        has-kernel: root.perf-vector-has-kernel;
                                        stroke-color: root.perf-vector-color;
                                        fill-color: root.perf-vector-fill-color;
                                        show-grid: root.custom-show-grid;
                                        title-label: root.perf-main-graph-label;
                                        max-label: root.perf-vector-max-label;
                                        min-label: root.perf-vector-min-label;
                                        current-val-label: root.perf-vector-current-val;
                                    }
                                    if root.perf-has-secondary-graph : VectorTelemetryChart {
                                        vertical-stretch: 1;
                                        stroke-path: root.perf-sec-vector-stroke;
                                        fill-path: root.perf-sec-vector-fill;
                                        has-kernel: false;
                                        stroke-color: root.perf-sec-vector-color;
                                        fill-color: root.perf-sec-vector-fill-color;
                                        show-grid: root.custom-show-grid;
                                        title-label: root.perf-sec-graph-label;
                                        max-label: root.perf-sec-vector-max-label;
                                        min-label: root.perf-sec-vector-min-label;
                                        current-val-label: root.perf-sec-vector-current-val;
                                    }
                                }
                            }

                            // Bottom Telemetry Readout Grid (Matches Screenshots 2 & 3)
                            HorizontalLayout {
                                height: 185px;
                                padding-top: 6px;
                                spacing: 24px;

                                // Left Metric Columns
                                VerticalLayout {
                                    horizontal-stretch: 1;
                                    spacing: 10px;

                                    HorizontalLayout {
                                        spacing: 24px;
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m1-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m1-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m2-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m2-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m3-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m3-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m4-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m4-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                    }

                                    HorizontalLayout {
                                        spacing: 24px;
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m5-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m5-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                        if root.perf-m6-label != "" : VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m6-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m6-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                        if root.perf-m7-label != "" : VerticalLayout {
                                            horizontal-stretch: 1;
                                            Text { text: root.perf-m7-label; color: Theme.text-dim; font-size: 11px; }
                                            Text { text: root.perf-m7-val; color: Theme.text-main; font-size: 15px; }
                                        }
                                    }

                                    // Key-Value Hardware Metadata Grid
                                    HorizontalLayout {
                                        spacing: 32px;
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            spacing: 4px;
                                            HorizontalLayout { Text { width: 130px; text: root.perf-kv1-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv1-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 130px; text: root.perf-kv2-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv2-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 130px; text: root.perf-kv3-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv3-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 130px; text: root.perf-kv4-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv4-v; color: Theme.text-main; font-size: 11px; } }
                                        }
                                        VerticalLayout {
                                            horizontal-stretch: 1;
                                            spacing: 4px;
                                            HorizontalLayout { Text { width: 140px; text: root.perf-kv5-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv5-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 140px; text: root.perf-kv6-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv6-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 140px; text: root.perf-kv7-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv7-v; color: Theme.text-main; font-size: 11px; } }
                                            HorizontalLayout { Text { width: 140px; text: root.perf-kv8-k; color: Theme.text-dim; font-size: 11px; } Text { text: root.perf-kv8-v; color: Theme.text-main; font-size: 11px; } }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 2: PROCESSES
                    // ============================================================
                    if root.active-tab == 2 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;

                            HorizontalLayout {
                                padding: 8px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "Filter:"; color: Theme.text-dim; vertical-alignment: center; }
                                LineEdit {
                                    width: 240px;
                                    placeholder-text: "Name, user, PID, cmdline...";
                                    text <=> root.proc-filter;
                                    edited => { root.refresh-filters(); }
                                }
                                ActionBtn {
                                    label: root.proc-tree-view ? "Mode: Tree" : "Mode: List";
                                    stroke-col: root.proc-tree-view ? Theme.accent-primary : Theme.border;
                                    clicked => { root.proc-tree-view = !root.proc-tree-view; root.refresh-filters(); }
                                }
                                ActionBtn {
                                    label: root.proc-hide-kthreads ? "User Only" : "All + Kernel";
                                    stroke-col: root.proc-hide-kthreads ? Theme.accent-secondary : Theme.border;
                                    clicked => { root.proc-hide-kthreads = !root.proc-hide-kthreads; root.refresh-filters(); }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 2;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;

                            VerticalLayout {
                                padding: 8px;
                                spacing: 4px;
                                Rectangle {
                                    height: 24px;
                                    background: Theme.bg-header;
                                    HorizontalLayout {
                                        padding-left: 6px;
                                        padding-right: 6px;
                                        spacing: 6px;
                                        Text { width: 58px; text: "PID"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 58px; text: "PPID"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 220px; text: "PROCESS"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 85px; text: "USER"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 70px; text: "CPU %"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 85px; text: "MEMORY"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 80px; text: "DISK R/s"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 80px; text: "DISK W/s"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "ACTIONS"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    }
                                }
                                ScrollView {
                                    vertical-stretch: 1;
                                    VerticalLayout {
                                        spacing: 2px;
                                        for p[idx] in root.processes : Rectangle {
                                            height: Theme.row-height;
                                            background: row-ta.has-hover ? Theme.menu-hover : (mod(idx, 2) == 0 ? transparent : Theme.row-alt);
                                            border-radius: Theme.button-radius;

                                            row-ta := TouchArea {
                                                mouse-cursor: pointer;
                                                pointer-event(event) => {
                                                    if (event.button == PointerEventButton.right && event.kind == PointerEventKind.down) {
                                                        root.proc-context-pid = p.pid;
                                                        root.proc-context-name = p.name;
                                                        root.proc-context-cmdline = p.cmdline;
                                                        root.proc-context-x = 480px;
                                                        root.proc-context-y = 240px;
                                                        root.proc-context-open = true;
                                                    }
                                                }
                                                double-clicked => {
                                                    root.inspect-process(p.pid);
                                                }
                                            }

                                            HorizontalLayout {
                                                padding-left: 6px;
                                                padding-right: 6px;
                                                spacing: 6px;
                                                Text { width: 58px; text: p.pid; color: Theme.accent-secondary; vertical-alignment: center; }
                                                Text { width: 58px; text: p.ppid; color: Theme.text-dim; vertical-alignment: center; }
                                                Text { width: 220px; text: p.display_name; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                Text { width: 85px; text: p.user; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                                Text { width: 70px; text: p.cpu_str; color: Theme.accent-primary; vertical-alignment: center; }
                                                Text { width: 85px; text: p.rss_str; color: Theme.text-main; vertical-alignment: center; }
                                                Text { width: 80px; text: p.io_read_str; color: Theme.text-dim; vertical-alignment: center; }
                                                Text { width: 80px; text: p.io_write_str; color: Theme.text-dim; vertical-alignment: center; }
                                                HorizontalLayout {
                                                    horizontal-stretch: 1;
                                                    spacing: 4px;
                                                    alignment: start;
                                                    ActionBtn { label: "Inspect"; clicked => { root.inspect-process(p.pid); } }
                                                    ActionBtn { label: "End Task"; stroke-col: Theme.accent-danger; clicked => { root.signal-process(p.pid, "TERM"); } }
                                                    ActionBtn {
                                                        label: "⋮";
                                                        clicked => {
                                                            root.proc-context-pid = p.pid;
                                                            root.proc-context-name = p.name;
                                                            root.proc-context-cmdline = p.cmdline;
                                                            root.proc-context-x = 550px;
                                                            root.proc-context-y = 250px;
                                                            root.proc-context-open = true;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        Rectangle {
                            height: 185px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 10px;
                                spacing: 4px;
                                Text { text: root.inspector-title; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                ScrollView {
                                    vertical-stretch: 1;
                                    VerticalLayout {
                                        Text {
                                            text: root.inspector-body;
                                            color: Theme.text-main;
                                            font-size: Theme.font-size-sm;
                                            wrap: word-wrap;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 3: SYSTEM INFO
                    // ============================================================
                    if root.active-tab == 3 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 12px;
                                alignment: start;
                                ActionBtn { label: "Export System Info Report"; stroke-col: Theme.accent-primary; clicked => { root.export-sysinfo(); } }
                                Text { text: "Hardware Overview, Hybrid CPU Topology, Storage, Graphics & Telemetry Providers"; color: Theme.text-dim; vertical-alignment: center; }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 1;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            ScrollView {
                                VerticalLayout {
                                    padding: 12px;
                                    Text {
                                        text: root.sysinfo-text;
                                        color: Theme.text-main;
                                        font-family: Theme.font-family;
                                        font-size: Theme.font-size-base;
                                        wrap: word-wrap;
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TABS 4, 5, 6, 10: CATALOGS (Startup apps, Users, Services, Installed Apps)
                    // ============================================================
                    if root.active-tab == 4 || root.active-tab == 5 || root.active-tab == 6 || root.active-tab == 10 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "Filter:"; color: Theme.text-dim; vertical-alignment: center; }
                                LineEdit {
                                    width: 300px;
                                    placeholder-text: "Search items...";
                                    text <=> root.catalog-filter;
                                    edited => { root.refresh-filters(); }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 1;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 8px;
                                spacing: 4px;
                                Rectangle {
                                    height: 24px;
                                    background: Theme.bg-header;
                                    HorizontalLayout {
                                        padding-left: 6px;
                                        padding-right: 6px;
                                        spacing: 8px;
                                        Text { width: 220px; text: "NAME / UNIT"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 120px; text: "VERSION / STATE"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 110px; text: "SOURCE / SUB"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 120px; text: "CATEGORY / PID"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 100px; text: "SIZE / METRIC"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "DETAILS"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    }
                                }
                                ScrollView {
                                    vertical-stretch: 1;
                                    VerticalLayout {
                                        spacing: 2px;
                                        for row[idx] in root.catalog-rows : Rectangle {
                                            height: Theme.row-height;
                                            background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                            HorizontalLayout {
                                                padding-left: 6px;
                                                padding-right: 6px;
                                                spacing: 8px;
                                                Text { width: 220px; text: row.col1; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                Text { width: 120px; text: row.col2; color: Theme.accent-primary; vertical-alignment: center; overflow: elide; }
                                                Text { width: 110px; text: row.col3; color: Theme.accent-secondary; vertical-alignment: center; overflow: elide; }
                                                Text { width: 120px; text: row.col4; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                                Text { width: 100px; text: row.col5; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                Text { horizontal-stretch: 1; text: row.col6; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 7: POWER & FREQ
                    // ============================================================
                    if root.active-tab == 7 : Rectangle {
                        vertical-stretch: 1;
                        background: Theme.bg-card;
                        border-width: 1px;
                        border-color: Theme.border;
                        border-radius: 6px;

                        VerticalLayout {
                            padding: 10px;
                            spacing: 6px;
                            Text { text: "HARDWARE TELEMETRY SENSORS (CPU CLOCKS, TEMPERATURES, RAPL POWER, VOLTAGES & FANS)"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                            Rectangle {
                                height: 24px;
                                background: Theme.bg-header;
                                HorizontalLayout {
                                    padding-left: 6px;
                                    padding-right: 6px;
                                    spacing: 8px;
                                    Text { width: 150px; text: "CHIP / GROUP"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 120px; text: "ADAPTER"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 220px; text: "SENSOR"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 110px; text: "CATEGORY"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 110px; text: "CURRENT"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 110px; text: "MIN"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { width: 110px; text: "MAX"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    Text { horizontal-stretch: 1; text: "LEVEL"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                }
                            }
                            ScrollView {
                                vertical-stretch: 1;
                                VerticalLayout {
                                    spacing: 2px;
                                    for s[idx] in root.sensors : Rectangle {
                                        height: Theme.row-height;
                                        background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                        HorizontalLayout {
                                            padding-left: 6px;
                                            padding-right: 6px;
                                            spacing: 8px;
                                            Text { width: 150px; text: s.chip; color: Theme.accent-secondary; vertical-alignment: center; overflow: elide; }
                                            Text { width: 120px; text: s.adapter; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                            Text { width: 220px; text: s.label; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                            Text { width: 110px; text: s.category; color: Theme.text-dim; vertical-alignment: center; }
                                            Text { width: 110px; text: s.cur_str; color: Theme.accent-primary; font-weight: 700; vertical-alignment: center; }
                                            Text { width: 110px; text: s.min_str; color: Theme.text-dim; vertical-alignment: center; }
                                            Text { width: 110px; text: s.max_str; color: Theme.accent-warn; vertical-alignment: center; }
                                            VerticalLayout {
                                                horizontal-stretch: 1;
                                                alignment: center;
                                                BarMeter { ratio: s.ratio; fill-color: Theme.accent-primary; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 8: FLIGHT RECORDER
                    // ============================================================
                    if root.active-tab == 8 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 46px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 8px;
                                alignment: start;
                                ActionBtn { label: "|< First"; clicked => { root.scrub-flight(0); } }
                                ActionBtn { label: "<< -10"; clicked => { root.scrub-flight(root.flight-scrub-index - 10); } }
                                ActionBtn { label: "< Prev"; clicked => { root.scrub-flight(root.flight-scrub-index - 1); } }
                                ActionBtn { label: "Next >"; clicked => { root.scrub-flight(root.flight-scrub-index + 1); } }
                                ActionBtn { label: "+10 >>"; clicked => { root.scrub-flight(root.flight-scrub-index + 10); } }
                                ActionBtn { label: "Latest >|"; stroke-col: Theme.accent-secondary; clicked => { root.scrub-flight(999999); } }
                                ActionBtn { label: "Export .pulsetrace"; stroke-col: Theme.accent-primary; clicked => { root.export-flight-trace(); } }
                            }
                        }

                        Rectangle {
                            height: 220px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 10px;
                                spacing: 6px;
                                Text { text: "FLIGHT RECORDER MULTI-METRIC TIMELINE (CPU %, MEMORY %, SYSTEM PRESSURE & SCRUB MARKER)"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                Image { source: root.flight-graph; image-fit: fill; vertical-stretch: 1; }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 1;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 14px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "INSPECTED TELEMETRY FRAME DETAILS"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                Rectangle { height: 1px; background: Theme.border; }
                                Text { text: root.flight-frame-summary; color: Theme.text-main; font-size: Theme.font-size-base; wrap: word-wrap; }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 9: CONNECTIONS
                    // ============================================================
                    if root.active-tab == 9 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "Filter:"; color: Theme.text-dim; vertical-alignment: center; }
                                LineEdit {
                                    width: 260px;
                                    placeholder-text: "Protocol, IP, port, process...";
                                    text <=> root.conn-filter;
                                    edited => { root.refresh-filters(); }
                                }
                                ActionBtn {
                                    label: root.conn-public-only ? "Public Listeners Only" : "All Sockets";
                                    stroke-col: root.conn-public-only ? Theme.accent-warn : Theme.border;
                                    clicked => { root.conn-public-only = !root.conn-public-only; root.refresh-filters(); }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 1;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 8px;
                                spacing: 4px;
                                Rectangle {
                                    height: 24px;
                                    background: Theme.bg-header;
                                    HorizontalLayout {
                                        padding-left: 6px;
                                        padding-right: 6px;
                                        spacing: 8px;
                                        Text { width: 70px; text: "PROTO"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 220px; text: "LOCAL ENDPOINT"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 220px; text: "REMOTE ENDPOINT"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 110px; text: "STATE"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 70px; text: "PID"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 180px; text: "PROCESS"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "EXPOSURE"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    }
                                }
                                ScrollView {
                                    vertical-stretch: 1;
                                    VerticalLayout {
                                        spacing: 2px;
                                        for c[idx] in root.connections : Rectangle {
                                            height: Theme.row-height;
                                            background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                            HorizontalLayout {
                                                padding-left: 6px;
                                                padding-right: 6px;
                                                spacing: 8px;
                                                Text { width: 70px; text: c.proto; color: Theme.accent-secondary; vertical-alignment: center; }
                                                Text { width: 220px; text: c.local_ep; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                Text { width: 220px; text: c.remote_ep; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                                Text { width: 110px; text: c.state; color: Theme.accent-primary; vertical-alignment: center; }
                                                Text { width: 70px; text: c.pid_str; color: Theme.accent-secondary; vertical-alignment: center; }
                                                Text { width: 180px; text: c.proc_name; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                Text { horizontal-stretch: 1; text: c.public_badge; color: c.is_public ? Theme.accent-warn : Theme.text-dim; vertical-alignment: center; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 11: DRIVERS
                    // ============================================================
                    if root.active-tab == 11 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "Filter Driver:"; color: Theme.text-dim; vertical-alignment: center; }
                                LineEdit {
                                    width: 240px;
                                    placeholder-text: "Module name, dependency, path...";
                                    text <=> root.drv-filter;
                                    edited => { root.refresh-filters(); }
                                }
                                Text { text: root.drv-summary; color: Theme.accent-primary; vertical-alignment: center; }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 1;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 8px;
                                spacing: 4px;
                                Rectangle {
                                    height: 24px;
                                    background: Theme.bg-header;
                                    HorizontalLayout {
                                        padding-left: 6px;
                                        padding-right: 6px;
                                        spacing: 8px;
                                        Text { width: 160px; text: "MODULE"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 90px; text: "MEMORY"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 65px; text: "REFS"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 85px; text: "STATE"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 85px; text: "ORIGIN"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { width: 220px; text: "USED BY"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                        Text { horizontal-stretch: 1; text: "MODULE PATH"; color: Theme.text-dim; font-size: Theme.font-size-sm; font-weight: 700; vertical-alignment: center; }
                                    }
                                }
                                ScrollView {
                                    vertical-stretch: 1;
                                    VerticalLayout {
                                        spacing: 2px;
                                        for m[idx] in root.drivers : Rectangle {
                                            height: Theme.row-height;
                                            background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                            HorizontalLayout {
                                                padding-left: 6px;
                                                padding-right: 6px;
                                                spacing: 8px;
                                                Text { width: 160px; text: m.name; color: Theme.accent-primary; vertical-alignment: center; overflow: elide; }
                                                Text { width: 90px; text: m.size_str; color: Theme.text-main; vertical-alignment: center; }
                                                Text { width: 65px; text: m.refs_str; color: Theme.accent-secondary; vertical-alignment: center; }
                                                Text { width: 85px; text: m.state; color: Theme.text-main; vertical-alignment: center; }
                                                Text { width: 85px; text: m.in_tree; color: Theme.text-dim; vertical-alignment: center; }
                                                Text { width: 220px; text: m.used_by; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                                Text { horizontal-stretch: 1; text: m.path; color: Theme.text-dim; vertical-alignment: center; overflow: elide; }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 12: DISK SPACE
                    // ============================================================
                    if root.active-tab == 12 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 10px;

                        Rectangle {
                            height: 44px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 10px;
                                alignment: start;
                                Text { text: "Target Path:"; color: Theme.text-dim; vertical-alignment: center; }
                                LineEdit { width: 320px; text <=> root.disk-scan-path; }
                                ActionBtn { label: "Scan Directory"; stroke-col: Theme.accent-primary; clicked => { root.scan-disk-space(root.disk-scan-path); } }
                                Text { horizontal-stretch: 1; min-width: 0px; text: root.disk-scan-summary; color: Theme.accent-secondary; vertical-alignment: center; overflow: elide; }
                            }
                        }

                        HorizontalLayout {
                            vertical-stretch: 1;
                            spacing: 10px;
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 8px;
                                    spacing: 4px;
                                    Text { text: "TOP DIRECTORIES BY SIZE"; color: Theme.accent-primary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    ScrollView {
                                        vertical-stretch: 1;
                                        VerticalLayout {
                                            spacing: 2px;
                                            for d[idx] in root.disk-dirs : Rectangle {
                                                height: Theme.row-height;
                                                background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                                HorizontalLayout {
                                                    padding-left: 6px;
                                                    padding-right: 6px;
                                                    spacing: 8px;
                                                    Text { width: 36px; text: d.rank; color: Theme.text-dim; vertical-alignment: center; }
                                                    Text { width: 170px; text: d.name; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                    Text { width: 85px; text: d.size_str; color: Theme.accent-primary; vertical-alignment: center; }
                                                    Text { width: 60px; text: d.pct_str; color: Theme.accent-secondary; vertical-alignment: center; }
                                                    VerticalLayout { horizontal-stretch: 1; alignment: center; BarMeter { ratio: d.ratio; fill-color: Theme.accent-primary; } }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 8px;
                                    spacing: 4px;
                                    Text { text: "FILE TYPE CATEGORIES"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    ScrollView {
                                        vertical-stretch: 1;
                                        VerticalLayout {
                                            spacing: 2px;
                                            for c[idx] in root.disk-categories : Rectangle {
                                                height: Theme.row-height;
                                                background: mod(idx, 2) == 0 ? transparent : Theme.row-alt;
                                                HorizontalLayout {
                                                    padding-left: 6px;
                                                    padding-right: 6px;
                                                    spacing: 8px;
                                                    Text { width: 180px; text: c.name; color: Theme.text-main; vertical-alignment: center; overflow: elide; }
                                                    Text { width: 90px; text: c.size_str; color: Theme.accent-secondary; vertical-alignment: center; }
                                                    Text { width: 80px; text: c.files_str; color: Theme.text-dim; vertical-alignment: center; }
                                                    Text { width: 60px; text: c.pct_str; color: Theme.accent-primary; vertical-alignment: center; }
                                                    VerticalLayout { horizontal-stretch: 1; alignment: center; BarMeter { ratio: c.ratio; fill-color: Theme.accent-secondary; } }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // ============================================================
                    // TAB 13: BENCHMARKS
                    // ============================================================
                    if root.active-tab == 13 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 12px;

                        Rectangle {
                            height: 46px;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            HorizontalLayout {
                                padding: 8px;
                                spacing: 14px;
                                alignment: start;
                                ActionBtn { label: "Run All Hardware Benchmarks"; stroke-col: Theme.accent-primary; clicked => { root.run-benchmarks(); } }
                                Text { text: "Executes Single-Core CPU, Multi-Core CPU, Memory Bandwidth, and Storage I/O workloads (Baseline = 100)"; color: Theme.text-dim; vertical-alignment: center; }
                            }
                        }

                        HorizontalLayout {
                            spacing: 12px;
                            height: 150px;
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.accent-primary;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 14px;
                                    spacing: 8px;
                                    Text { text: "COMPOSITE PULSEDECK SCORE"; color: Theme.accent-primary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    Text { text: root.bench-composite; color: Theme.accent-primary; font-size: 32px; font-weight: 700; }
                                    Text { text: "Baseline Reference = 100"; color: Theme.text-dim; font-size: Theme.font-size-sm; }
                                }
                            }
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 14px;
                                    spacing: 8px;
                                    Text { text: "CPU SINGLE-CORE"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    Text { text: root.bench-sc; color: Theme.text-main; font-size: 28px; font-weight: 700; }
                                    Text { text: root.bench-sc-sub; color: Theme.text-dim; font-size: Theme.font-size-sm; }
                                }
                            }
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 14px;
                                    spacing: 8px;
                                    Text { text: "CPU MULTI-CORE"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    Text { text: root.bench-mc; color: Theme.text-main; font-size: 28px; font-weight: 700; }
                                    Text { text: root.bench-mc-sub; color: Theme.text-dim; font-size: Theme.font-size-sm; }
                                }
                            }
                            Rectangle {
                                horizontal-stretch: 1;
                                background: Theme.bg-card;
                                border-width: 1px;
                                border-color: Theme.border;
                                border-radius: 6px;
                                VerticalLayout {
                                    padding: 14px;
                                    spacing: 8px;
                                    Text { text: "MEMORY & STORAGE I/O"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                    Text { text: "MEM " + root.bench-mem + " / DISK " + root.bench-disk; color: Theme.text-main; font-size: 20px; font-weight: 700; }
                                    Text { text: root.bench-mem-sub + " | " + root.bench-disk-sub; color: Theme.text-dim; font-size: Theme.font-size-sm; }
                                }
                            }
                        }

                        Rectangle { vertical-stretch: 1; }
                    }

                    // ============================================================
                    // TABS 14 & 15: SETTINGS & COLORS STUDIO
                    // ============================================================
                    if root.active-tab == 14 || root.active-tab == 15 : VerticalLayout {
                        vertical-stretch: 1;
                        spacing: 12px;

                        Rectangle {
                            vertical-stretch: 0;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 14px;
                                spacing: 10px;
                                Text { text: "COLOUR THEME PRESETS"; color: Theme.accent-primary; font-size: Theme.font-size-sm; font-weight: 700; }
                                HorizontalLayout {
                                    spacing: 8px;
                                    alignment: start;
                                    ActionBtn { label: "Fluent Modern"; stroke-col: root.current-theme-name == "Fluent Modern" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Fluent Modern"); } }
                                    ActionBtn { label: "KDE Breeze"; stroke-col: root.current-theme-name == "KDE Breeze" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("KDE Breeze"); } }
                                    ActionBtn { label: "VFD Phosphor Green"; stroke-col: root.current-theme-name == "VFD Phosphor Green" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("VFD Phosphor Green"); } }
                                    ActionBtn { label: "Nordic Frost"; stroke-col: root.current-theme-name == "Nordic Frost" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Nordic Frost"); } }
                                }
                                HorizontalLayout {
                                    spacing: 8px;
                                    alignment: start;
                                    ActionBtn { label: "Solarized Daylight"; stroke-col: root.current-theme-name == "Solarized Daylight" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Solarized Daylight"); } }
                                    ActionBtn { label: "Cyberpunk Cyan"; stroke-col: root.current-theme-name == "Cyberpunk Cyan" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Cyberpunk Cyan"); } }
                                    ActionBtn { label: "Amber CRT"; stroke-col: root.current-theme-name == "Amber CRT" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Amber CRT"); } }
                                    ActionBtn { label: "Crimson Ops"; stroke-col: root.current-theme-name == "Crimson Ops" ? Theme.accent-primary : Theme.border; clicked => { root.apply-theme-preset("Crimson Ops"); } }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 0;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 14px;
                                spacing: 10px;
                                Text { text: "CUSTOM HEX COLOUR OVERRIDES (#RRGGBB)"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                HorizontalLayout {
                                    spacing: 12px;
                                    alignment: start;
                                    VerticalLayout {
                                        spacing: 4px;
                                        HorizontalLayout {
                                            spacing: 6px;
                                            Rectangle { width: 10px; height: 10px; y: (parent.height - self.height) / 2; border-radius: 2px; background: Theme.accent-primary; border-width: 1px; border-color: Theme.text-dim; }
                                            Text { text: "Primary Accent"; color: Theme.text-dim; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        }
                                        LineEdit { width: 115px; text <=> root.custom-primary-hex; }
                                    }
                                    VerticalLayout {
                                        spacing: 4px;
                                        HorizontalLayout {
                                            spacing: 6px;
                                            Rectangle { width: 10px; height: 10px; y: (parent.height - self.height) / 2; border-radius: 2px; background: Theme.accent-secondary; border-width: 1px; border-color: Theme.text-dim; }
                                            Text { text: "Secondary Accent"; color: Theme.text-dim; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        }
                                        LineEdit { width: 115px; text <=> root.custom-secondary-hex; }
                                    }
                                    VerticalLayout {
                                        spacing: 4px;
                                        HorizontalLayout {
                                            spacing: 6px;
                                            Rectangle { width: 10px; height: 10px; y: (parent.height - self.height) / 2; border-radius: 2px; background: Theme.bg-main; border-width: 1px; border-color: Theme.text-dim; }
                                            Text { text: "Main Background"; color: Theme.text-dim; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        }
                                        LineEdit { width: 115px; text <=> root.custom-bg-hex; }
                                    }
                                    VerticalLayout {
                                        spacing: 4px;
                                        HorizontalLayout {
                                            spacing: 6px;
                                            Rectangle { width: 10px; height: 10px; y: (parent.height - self.height) / 2; border-radius: 2px; background: Theme.bg-card; border-width: 1px; border-color: Theme.text-dim; }
                                            Text { text: "Card Background"; color: Theme.text-dim; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        }
                                        LineEdit { width: 115px; text <=> root.custom-card-hex; }
                                    }
                                    VerticalLayout {
                                        spacing: 4px;
                                        HorizontalLayout {
                                            spacing: 6px;
                                            Rectangle { width: 10px; height: 10px; y: (parent.height - self.height) / 2; border-radius: 2px; background: Theme.text-main; border-width: 1px; border-color: Theme.text-dim; }
                                            Text { text: "Primary Text"; color: Theme.text-dim; font-size: Theme.font-size-sm; vertical-alignment: center; }
                                        }
                                        LineEdit { width: 115px; text <=> root.custom-text-hex; }
                                    }
                                    VerticalLayout {
                                        alignment: end;
                                        ActionBtn { label: "Apply Custom Hex Colours"; stroke-col: Theme.accent-primary; clicked => { root.apply-custom-colors(); } }
                                    }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 0;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 14px;
                                spacing: 10px;
                                Text { text: "TYPOGRAPHY, FONT FAMILY & DENSITY"; color: Theme.accent-primary; font-size: Theme.font-size-sm; font-weight: 700; }
                                HorizontalLayout {
                                    spacing: 8px;
                                    alignment: start;
                                    ActionBtn { label: "Sans-Serif"; stroke-col: root.custom-font-family == "sans-serif" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("sans-serif"); } }
                                    ActionBtn { label: "Noto Sans"; stroke-col: root.custom-font-family == "Noto Sans" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("Noto Sans"); } }
                                    ActionBtn { label: "Segoe UI"; stroke-col: root.custom-font-family == "Segoe UI" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("Segoe UI"); } }
                                    ActionBtn { label: "DejaVu Sans Mono"; stroke-col: root.custom-font-family == "DejaVu Sans Mono" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("DejaVu Sans Mono"); } }
                                    ActionBtn { label: "JetBrains Mono"; stroke-col: root.custom-font-family == "JetBrains Mono" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("JetBrains Mono"); } }
                                    ActionBtn { label: "Fira Code"; stroke-col: root.custom-font-family == "Fira Code" ? Theme.accent-primary : Theme.border; clicked => { root.select-font-family("Fira Code"); } }
                                }
                                HorizontalLayout {
                                    spacing: 12px;
                                    alignment: start;
                                    Text { text: "Custom Font:"; color: Theme.text-dim; vertical-alignment: center; }
                                    LineEdit { width: 200px; text <=> root.custom-font-family; edited => { root.select-font-family(root.custom-font-family); } }
                                    Text { text: "Font Size (" + root.custom-font-size + "px):"; color: Theme.text-dim; vertical-alignment: center; }
                                    ActionBtn { label: "A-"; clicked => { root.adjust-font-size(-1); } }
                                    ActionBtn { label: "A+"; clicked => { root.adjust-font-size(1); } }
                                    ActionBtn {
                                        label: root.custom-compact-rows ? "Row Density: Compact" : "Row Density: Standard";
                                        stroke-col: root.custom-compact-rows ? Theme.accent-secondary : Theme.border;
                                        clicked => { root.toggle-compact-rows(); }
                                    }
                                    ActionBtn {
                                        label: root.custom-show-grid ? "Graph Grid: ON" : "Graph Grid: OFF";
                                        stroke-col: root.custom-show-grid ? Theme.accent-primary : Theme.border;
                                        clicked => { root.toggle-graph-grid(); }
                                    }
                                    ActionBtn {
                                        label: root.chart-fps == 0 ? "Smooth Charts: OFF" : "Smooth Charts: " + root.chart-fps + " FPS";
                                        stroke-col: root.chart-fps > 0 ? Theme.accent-primary : Theme.border;
                                        clicked => { root.cycle-chart-fps(); }
                                    }
                                }
                            }
                        }

                        Rectangle {
                            vertical-stretch: 0;
                            background: Theme.bg-card;
                            border-width: 1px;
                            border-color: Theme.border;
                            border-radius: 6px;
                            VerticalLayout {
                                padding: 14px;
                                spacing: 10px;
                                Text { text: "TELEMETRY REFRESH INTERVAL & PERSISTENCE"; color: Theme.accent-secondary; font-size: Theme.font-size-sm; font-weight: 700; }
                                HorizontalLayout {
                                    spacing: 10px;
                                    alignment: start;
                                    ActionBtn { label: "250 ms (Fast)"; stroke-col: root.custom-refresh-ms == 250 ? Theme.accent-primary : Theme.border; clicked => { root.set-refresh-rate(250); } }
                                    ActionBtn { label: "500 ms"; stroke-col: root.custom-refresh-ms == 500 ? Theme.accent-primary : Theme.border; clicked => { root.set-refresh-rate(500); } }
                                    ActionBtn { label: "1000 ms (1s)"; stroke-col: root.custom-refresh-ms == 1000 ? Theme.accent-primary : Theme.border; clicked => { root.set-refresh-rate(1000); } }
                                    ActionBtn { label: "2000 ms (2s)"; stroke-col: root.custom-refresh-ms == 2000 ? Theme.accent-primary : Theme.border; clicked => { root.set-refresh-rate(2000); } }
                                    ActionBtn { label: "Save Settings to Disk"; stroke-col: Theme.accent-primary; clicked => { root.save-settings(); } }
                                    ActionBtn { label: "Reset to Defaults"; stroke-col: Theme.accent-danger; clicked => { root.reset-settings(); } }
                                }
                            }
                        }

                        Rectangle { vertical-stretch: 1; }
                    }
                }
            }

            // 3. Bottom Status Bar (Exact Match to Screenshots 1–3)
            Rectangle {
                height: 28px;
                background: #0d1015;
                border-width: 1px;
                border-color: #171d26;

                HorizontalLayout {
                    padding-left: 12px;
                    padding-right: 12px;
                    alignment: space-between;

                    HorizontalLayout {
                        spacing: 6px;
                        alignment: start;
                        Text { text: "●"; color: #22e58b; font-size: 12px; vertical-alignment: center; }
                        Text { text: root.status-left-text + " · "; color: Theme.text-main; font-size: Theme.font-size-sm; vertical-alignment: center; }
                        Text { text: root.status-ip-text; color: #19b5fe; font-size: Theme.font-size-sm; vertical-alignment: center; }
                        Text { text: " · " + root.status-gen-text; color: Theme.text-main; font-size: Theme.font-size-sm; vertical-alignment: center; }
                        if root.status-message != "" : Text {
                            text: "  [" + root.status-message + "]";
                            color: Theme.accent-primary;
                            font-size: Theme.font-size-sm;
                            vertical-alignment: center;
                        }
                    }

                    // Right Recording Controls & VFD Timer
                    HorizontalLayout {
                        spacing: 8px;
                        alignment: end;
                        Text { text: "●"; color: #8c2323; font-size: 12px; vertical-alignment: center; }
                        Rectangle {
                            width: 24px;
                            height: 20px;
                            border-radius: 4px;
                            background: #141920;
                            border-width: 1px;
                            border-color: #262f3d;
                            Text { text: "■"; color: Theme.text-dim; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                            TouchArea { mouse-cursor: pointer; clicked => { root.toggle-flight-recording(false); } }
                        }
                        Rectangle {
                            width: 24px;
                            height: 20px;
                            border-radius: 4px;
                            background: #141920;
                            border-width: 1px;
                            border-color: #262f3d;
                            Text { text: "●"; color: #e03e3e; font-size: 10px; horizontal-alignment: center; vertical-alignment: center; }
                            TouchArea { mouse-cursor: pointer; clicked => { root.toggle-flight-recording(true); } }
                        }
                        Rectangle {
                            width: 86px;
                            height: 20px;
                            background: #070a0d;
                            border-width: 1px;
                            border-color: #1c2430;
                            border-radius: 3px;
                            Text {
                                text: root.status-timer-text;
                                color: #36e8ff;
                                font-family: Theme.font-family;
                                font-size: 11px;
                                horizontal-alignment: center;
                                vertical-alignment: center;
                            }
                        }
                    }
                }
            }
        }

        // Top Menu Dropdown Overlay & Click-Away Backdrop
        if root.open-menu != 0 : TouchArea {
            x: 0px;
            y: 28px;
            width: root.width;
            height: root.height - 28px;
            clicked => { root.open-menu = 0; }
        }

        // 1. FILE MENU DROPDOWN
        if root.open-menu == 1 : MenuDropdownContainer {
            x: 6px;
            y: 28px;
            menu-width: 295px;

            MenuDropdownItem {
                icon: "↻";
                label: "Refresh Telemetry Now";
                shortcut: "F5";
                triggered => { root.open-menu = 0; root.refresh-now(); }
            }
            MenuDropdownItem {
                icon: "🧭";
                label: "Export Flight Trace (.pulsetrace)";
                shortcut: "Ctrl+E";
                triggered => { root.open-menu = 0; root.export-flight-trace(); }
            }
            MenuDropdownItem {
                icon: "ⓘ";
                label: "Export System Info Report (.txt)";
                shortcut: "Ctrl+I";
                triggered => { root.open-menu = 0; root.export-sysinfo(); }
            }
            MenuDropdownItem {
                icon: "💾";
                label: "Save Settings & Theme";
                shortcut: "Ctrl+S";
                triggered => { root.open-menu = 0; root.save-settings(); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "🛡️";
                label: root.is-root-user ? "Running Elevated as Root" : "Relaunch as Root (pkexec)";
                triggered => { root.open-menu = 0; root.relaunch-as-root(); }
            }
            MenuDropdownItem {
                icon: "🪟";
                label: "Set as Default Task Manager (Win)";
                triggered => { root.open-menu = 0; root.toggle-windows-default-taskmgr(true); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "⏻";
                label: "Exit PulseDeck";
                shortcut: "Ctrl+Q";
                triggered => { root.open-menu = 0; root.quit-app(); }
            }
        }

        // 2. EDIT MENU DROPDOWN
        if root.open-menu == 2 : MenuDropdownContainer {
            x: 52px;
            y: 28px;
            menu-width: 280px;

            MenuDropdownItem {
                icon: "🔍";
                label: "Filter Processes...";
                triggered => { root.open-menu = 0; root.select-tab(2, "Processes"); }
            }
            MenuDropdownItem {
                icon: "🌐";
                label: "Filter Network Connections...";
                triggered => { root.open-menu = 0; root.select-tab(9, "Connections"); }
            }
            MenuDropdownItem {
                icon: "⚙";
                label: "Filter Kernel Drivers...";
                triggered => { root.open-menu = 0; root.select-tab(11, "Drivers"); }
            }
            MenuDropdownItem {
                icon: "🧹";
                label: "Clear All Active Search Filters";
                triggered => {
                    root.open-menu = 0;
                    root.proc-filter = "";
                    root.conn-filter = "";
                    root.drv-filter = "";
                    root.catalog-filter = "";
                    root.refresh-filters();
                }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "⚙";
                label: "Settings & Preferences...";
                triggered => { root.open-menu = 0; root.select-tab(14, "Settings"); }
            }
            MenuDropdownItem {
                icon: "🖌";
                label: "Colors & Theme Studio...";
                triggered => { root.open-menu = 0; root.select-tab(15, "Colors & Theme"); }
            }
        }

        // 3. VIEW MENU DROPDOWN
        if root.open-menu == 3 : MenuDropdownContainer {
            x: 104px;
            y: 28px;
            menu-width: 290px;

            MenuDropdownItem {
                icon: "⌂";
                label: "Summary Dashboard";
                triggered => { root.open-menu = 0; root.select-tab(0, "Summary"); }
            }
            MenuDropdownItem {
                icon: "📈";
                label: "Performance Observatory";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); }
            }
            MenuDropdownItem {
                icon: "📊";
                label: "Processes (Toggle Tree/List)";
                triggered => {
                    root.open-menu = 0;
                    root.proc-tree-view = !root.proc-tree-view;
                    root.select-tab(2, "Processes");
                }
            }
            MenuDropdownItem {
                icon: "⚡";
                label: "Power & Frequency Sensors";
                triggered => { root.open-menu = 0; root.select-tab(7, "Power & Freq"); }
            }
            MenuDropdownItem {
                icon: "🖴";
                label: "Disk Space Analyzer";
                triggered => { root.open-menu = 0; root.select-tab(12, "Disk Space"); }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Hardware Benchmarks";
                triggered => { root.open-menu = 0; root.select-tab(13, "Benchmarks"); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "🎨";
                label: "Theme: Fluent Modern";
                shortcut: root.current-theme-name == "Fluent Modern" ? "✓" : "";
                triggered => { root.open-menu = 0; root.apply-theme-preset("Fluent Modern"); }
            }
            MenuDropdownItem {
                icon: "🎨";
                label: "Theme: KDE Breeze";
                shortcut: root.current-theme-name == "KDE Breeze" ? "✓" : "";
                triggered => { root.open-menu = 0; root.apply-theme-preset("KDE Breeze"); }
            }
            MenuDropdownItem {
                icon: "🎨";
                label: "Theme: VFD Green";
                shortcut: root.current-theme-name == "VFD Phosphor Green" ? "✓" : "";
                triggered => { root.open-menu = 0; root.apply-theme-preset("VFD Phosphor Green"); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "◧";
                label: root.show-sidebar ? "Hide Navigation Sidebar" : "Show Navigation Sidebar";
                triggered => { root.open-menu = 0; root.show-sidebar = !root.show-sidebar; }
            }
            MenuDropdownItem {
                icon: "▦";
                label: "Graph Gridlines";
                shortcut: root.custom-show-grid ? "ON" : "OFF";
                triggered => { root.open-menu = 0; root.toggle-graph-grid(); }
            }
            MenuDropdownItem {
                icon: "≈";
                label: "Smooth Charts";
                shortcut: root.chart-fps == 0 ? "OFF" : root.chart-fps + " FPS";
                triggered => { root.open-menu = 0; root.cycle-chart-fps(); }
            }
            MenuDropdownItem {
                icon: "↕";
                label: "Table Density";
                shortcut: root.custom-compact-rows ? "Compact" : "Standard";
                triggered => { root.open-menu = 0; root.toggle-compact-rows(); }
            }
            MenuDropdownItem {
                icon: "A+";
                label: "Increase Font Size";
                triggered => { root.open-menu = 0; root.adjust-font-size(1); }
            }
            MenuDropdownItem {
                icon: "A-";
                label: "Decrease Font Size";
                triggered => { root.open-menu = 0; root.adjust-font-size(-1); }
            }
        }

        // 4. PERFORMANCE MENU DROPDOWN
        if root.open-menu == 4 : MenuDropdownContainer {
            x: 160px;
            y: 28px;
            menu-width: 330px;

            MenuDropdownItem {
                icon: "⚙";
                label: "CPU Utilization & Topology";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(0); }
            }
            MenuDropdownItem {
                icon: "⊞";
                label: root.perf-cpu-expanded ? "Switch to Overall Graph" : "Switch to Per-Core Grid";
                shortcut: root.perf-cpu-expanded ? "Per-Core" : "Overall";
                triggered => {
                    root.open-menu = 0;
                    root.perf-cpu-expanded = !root.perf-cpu-expanded;
                    root.select-tab(1, "Performance");
                    root.select-perf-item(0);
                }
            }
            MenuDropdownItem {
                icon: "▦";
                label: "Memory & Swap";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(1); }
            }
            MenuDropdownItem {
                icon: "🖴";
                label: "Physical Disks";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(2); }
            }
            MenuDropdownItem {
                icon: "🌐";
                label: "Network Adapters";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(3); }
            }
            MenuDropdownItem {
                icon: "🖥";
                label: "GPU Device";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(4); }
            }
            MenuDropdownItem {
                icon: "⚡";
                label: "Whole Machine Energy";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(5); }
            }
            MenuDropdownItem {
                icon: "🌡";
                label: "Whole Machine Thermals";
                triggered => { root.open-menu = 0; root.select-tab(1, "Performance"); root.select-perf-item(6); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: Fast (250 ms)";
                shortcut: root.custom-refresh-ms == 250 ? "✓" : "";
                triggered => { root.open-menu = 0; root.set-refresh-rate(250); }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: High (500 ms)";
                shortcut: root.custom-refresh-ms == 500 ? "✓" : "";
                triggered => { root.open-menu = 0; root.set-refresh-rate(500); }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: Normal (1000 ms)";
                shortcut: root.custom-refresh-ms == 1000 ? "✓" : "";
                triggered => { root.open-menu = 0; root.set-refresh-rate(1000); }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: Low (2000 ms)";
                shortcut: root.custom-refresh-ms == 2000 ? "✓" : "";
                triggered => { root.open-menu = 0; root.set-refresh-rate(2000); }
            }
        }

        // 5. HELP MENU DROPDOWN
        if root.open-menu == 5 : MenuDropdownContainer {
            x: 268px;
            y: 28px;
            menu-width: 270px;

            MenuDropdownItem {
                icon: "ⓘ";
                label: "System Diagnostic Report";
                triggered => { root.open-menu = 0; root.select-tab(3, "System Info"); }
            }
            MenuDropdownItem {
                icon: "🧭";
                label: "Flight Recorder Timeline";
                triggered => { root.open-menu = 0; root.select-tab(8, "Flight Recorder"); }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Run Hardware Benchmarks";
                triggered => { root.open-menu = 0; root.select-tab(13, "Benchmarks"); root.run-benchmarks(); }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "↺";
                label: "Reset Theme & Settings";
                triggered => { root.open-menu = 0; root.reset-settings(); }
            }
            MenuDropdownItem {
                icon: "★";
                label: "About PulseDeck v1.2.0";
                triggered => {
                    root.open-menu = 0;
                    root.status-message = "PulseDeck v1.2.0 — Native Slint Hardware Telemetry & System Observatory";
                    root.select-tab(3, "System Info");
                }
            }
        }

        // ====================================================================
        // FLOATING RIGHT-CLICK CONTEXT MENUS (Process & CPU Graph)
        // ====================================================================

        // Process Context Menu Backdrop
        if root.proc-context-open : TouchArea {
            x: 0px;
            y: 0px;
            width: root.width;
            height: root.height;
            clicked => { root.proc-context-open = false; }
        }

        // Process Context Menu
        if root.proc-context-open : MenuDropdownContainer {
            x: clamp(root.proc-context-x, 10px, root.width - 270px);
            y: clamp(root.proc-context-y, 32px, root.height - 360px);
            menu-width: 260px;

            Rectangle {
                height: 24px;
                HorizontalLayout {
                    padding-left: 8px;
                    padding-right: 8px;
                    alignment: space-between;
                    Text {
                        text: "PID " + root.proc-context-pid + " · " + root.proc-context-name;
                        color: Theme.accent-secondary;
                        font-family: Theme.font-ui;
                        font-size: 11px;
                        font-weight: 700;
                        overflow: elide;
                        vertical-alignment: center;
                    }
                }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "🔍";
                label: "Inspect Process Details";
                triggered => {
                    root.proc-context-open = false;
                    root.inspect-process(root.proc-context-pid);
                }
            }
            MenuDropdownItem {
                icon: "⏹";
                label: "End Task (SIGTERM)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "TERM");
                }
            }
            MenuDropdownItem {
                icon: "💥";
                label: "Force Kill (SIGKILL)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "KILL");
                }
            }
            MenuDropdownItem {
                icon: "⏸";
                label: "Pause Process (SIGSTOP)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "STOP");
                }
            }
            MenuDropdownItem {
                icon: "▶";
                label: "Resume Process (SIGCONT)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "CONT");
                }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "🔺";
                label: "Higher Priority (Nice -5)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "NICE_DEC");
                }
            }
            MenuDropdownItem {
                icon: "🔻";
                label: "Lower Priority (Nice +5)";
                triggered => {
                    root.proc-context-open = false;
                    root.signal-process(root.proc-context-pid, "NICE_INC");
                }
            }
            MenuDivider {}
            MenuDropdownItem {
                icon: "📋";
                label: "Copy Process PID";
                triggered => {
                    root.proc-context-open = false;
                    root.copy-to-clipboard(root.proc-context-pid);
                }
            }
            MenuDropdownItem {
                icon: "📋";
                label: "Copy Process Name";
                triggered => {
                    root.proc-context-open = false;
                    root.copy-to-clipboard(root.proc-context-name);
                }
            }
            MenuDropdownItem {
                icon: "📋";
                label: "Copy Command Line";
                triggered => {
                    root.proc-context-open = false;
                    root.copy-to-clipboard(root.proc-context-cmdline);
                }
            }
        }

        // Performance Telemetry Context Menu Backdrop
        if root.cpu-context-open : TouchArea {
            x: 0px;
            y: 0px;
            width: root.width;
            height: root.height;
            clicked => { root.cpu-context-open = false; }
        }

        // Performance Telemetry Context Menu
        if root.cpu-context-open : MenuDropdownContainer {
            x: clamp(root.cpu-context-x, 10px, root.width - 290px);
            y: clamp(root.cpu-context-y, 32px, root.height - 350px);
            menu-width: 280px;

            // 0 = CPU
            if root.perf-selected-idx == 0 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "CPU Telemetry Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "📈";
                    label: "Overall Utilization";
                    shortcut: !root.perf-cpu-expanded ? "✓" : "";
                    triggered => {
                        root.cpu-context-open = false;
                        root.perf-cpu-expanded = false;
                        root.refresh-now();
                    }
                }
                MenuDropdownItem {
                    icon: "⊞";
                    label: "Logical Processors (Per-Core)";
                    shortcut: root.perf-cpu-expanded ? "✓" : "";
                    triggered => {
                        root.cpu-context-open = false;
                        root.perf-cpu-expanded = true;
                        root.refresh-now();
                    }
                }
                MenuDropdownItem {
                    icon: "⚡";
                    label: "Copy CPU Topology & Specs";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard(root.perf-header-title + " · " + root.perf-header-pct + " · " + root.perf-header-sub + "\nSpeed: " + root.perf-m2-val + " · " + root.perf-kv1-k + " " + root.perf-kv1-v + "\nGovernor: " + root.perf-kv8-v);
                    }
                }
            }

            // 1 = Memory
            if root.perf-selected-idx == 1 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "Memory Telemetry Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "🧠";
                    label: "Memory In-Use vs Available";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "👥";
                    label: "View Process Memory Consumers";
                    shortcut: "Tab";
                    triggered => {
                        root.cpu-context-open = false;
                        root.select-tab(2, "Processes");
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy Memory Allocation Stats";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("Memory In Use: " + root.perf-m1-val + " / Total: " + root.perf-header-sub + "\nAvailable: " + root.perf-m2-val + " · Cached: " + root.perf-m4-val + "\nSwap: " + root.perf-m5-val);
                    }
                }
            }

            // 2 = Disk
            if root.perf-selected-idx == 2 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "Disk Telemetry Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "💾";
                    label: "Read / Write Throughput";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "🖴";
                    label: "Open Disk Space Analyzer";
                    shortcut: "Tab";
                    triggered => {
                        root.cpu-context-open = false;
                        root.select-tab(12, "Disk Space");
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy Disk Throughput Stats";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("Disk Activity: " + root.perf-header-pct + " · " + root.perf-sec-graph-right);
                    }
                }
            }

            // 3 = Network
            if root.perf-selected-idx == 3 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "Network Telemetry Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "🌐";
                    label: "Receive / Send Throughput";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "🔌";
                    label: "Open Network Connections";
                    shortcut: "Tab";
                    triggered => {
                        root.cpu-context-open = false;
                        root.select-tab(9, "Connections");
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy Network Adapter Info";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard(root.perf-header-title + " · " + root.perf-header-sub + "\nThroughput: " + root.perf-header-pct + " (" + root.perf-sec-graph-right + ")");
                    }
                }
            }

            // 4 = GPU
            if root.perf-selected-idx == 4 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "GPU Telemetry Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "🎮";
                    label: "3D Engine & Compute Usage";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "📼";
                    label: "VRAM Dedicated & Shared Details";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("GPU: " + root.perf-header-title + " (" + root.perf-header-pct + ")\nVRAM: " + root.perf-sec-graph-right + "\nDriver: " + root.perf-kv6-v);
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy GPU Telemetry Summary";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("GPU: " + root.perf-header-title + " | Usage: " + root.perf-header-pct + " | Temp: " + root.perf-m6-val + " | Power: " + root.perf-m5-val);
                    }
                }
            }

            // 5 = Energy
            if root.perf-selected-idx == 5 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "Energy & Power Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "⚡";
                    label: "Package Power Draw (Watts)";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "⚡";
                    label: "Open Power & Freq Inspector";
                    shortcut: "Tab";
                    triggered => {
                        root.cpu-context-open = false;
                        root.select-tab(7, "Power & Freq");
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy Power Consumption Stats";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("Instant Power Draw: " + root.perf-header-pct + " · " + root.perf-header-sub);
                    }
                }
            }

            // 6 = Thermals
            if root.perf-selected-idx == 6 : VerticalLayout {
                spacing: 2px;
                padding: 0px;
                Rectangle {
                    height: 24px;
                    HorizontalLayout {
                        padding-left: 8px;
                        padding-right: 8px;
                        Text { text: "Thermal Sensors Options"; color: Theme.accent-primary; font-family: Theme.font-ui; font-size: 11px; font-weight: 700; vertical-alignment: center; }
                    }
                }
                MenuDivider {}
                MenuDropdownItem {
                    icon: "🌡️";
                    label: "Package & Core Temperatures";
                    shortcut: "✓";
                    triggered => {
                        root.cpu-context-open = false;
                    }
                }
                MenuDropdownItem {
                    icon: "🩺";
                    label: "Open System Diagnostics Report";
                    shortcut: "Tab";
                    triggered => {
                        root.cpu-context-open = false;
                        root.select-tab(3, "System Info");
                    }
                }
                MenuDropdownItem {
                    icon: "📋";
                    label: "Copy Temperature Stats";
                    triggered => {
                        root.cpu-context-open = false;
                        root.copy-to-clipboard("Package Temp: " + root.perf-header-pct + " · " + root.perf-header-sub);
                    }
                }
            }

            // Common options for all Performance tabs
            MenuDivider {}
            MenuDropdownItem {
                icon: "▦";
                label: "Graph Gridlines";
                shortcut: root.custom-show-grid ? "ON" : "OFF";
                triggered => {
                    root.cpu-context-open = false;
                    root.toggle-graph-grid();
                }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: High (250 ms)";
                shortcut: root.custom-refresh-ms == 250 ? "✓" : "";
                triggered => {
                    root.cpu-context-open = false;
                    root.set-refresh-rate(250);
                }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: Normal (1000 ms)";
                shortcut: root.custom-refresh-ms == 1000 ? "✓" : "";
                triggered => {
                    root.cpu-context-open = false;
                    root.set-refresh-rate(1000);
                }
            }
            MenuDropdownItem {
                icon: "⏱";
                label: "Update Speed: Low (2000 ms)";
                shortcut: root.custom-refresh-ms == 2000 ? "✓" : "";
                triggered => {
                    root.cpu-context-open = false;
                    root.set-refresh-rate(2000);
                }
            }
        }
    }
}

const HISTORY_LEN: usize = 120;

struct GuiController {
    sampler: Arc<Mutex<LinuxSystemSampler>>,
    last_snap: SystemSnapshot,
    settings: AppSettings,
    cpu_hist: VecDeque<f64>,
    cpu_kern_hist: VecDeque<f64>,
    cpu_temp_hist: VecDeque<f64>,
    mem_hist: VecDeque<f64>,
    swap_hist: VecDeque<f64>,
    disk_r_hist: VecDeque<f64>,
    disk_w_hist: VecDeque<f64>,
    net_rx_hist: VecDeque<f64>,
    net_tx_hist: VecDeque<f64>,
    gpu_busy_hist: VecDeque<f64>,
    gpu_vram_hist: VecDeque<f64>,
    power_hist: VecDeque<f64>,
    core_total_hist: Vec<VecDeque<f64>>,
    core_kern_hist: Vec<VecDeque<f64>>,
    cpu_base_ghz: f64,
    cpu_driver: String,
    cpu_governor: String,
    // Long CPU history for the Summary "CPU HISTORY" chart, plus per-sample
    // process activity so a selected time range can be inspected afterwards.
    cpu_long: VecDeque<f64>,
    cpu_kern_long: VecDeque<f64>,
    cpu_temp_long: VecDeque<f64>,
    proc_frames: VecDeque<ProcFrame>,
    proc_names: HashMap<u32, Rc<str>>,
    /// Selected history range as inclusive sample sequence numbers.
    selection: Option<(u64, u64)>,
    /// Samples shown by the CPU history chart (a tail of the 10-min buffer).
    history_window: usize,
}

/// Number of samples kept for the CPU history chart (10 min at the default 1 s rate).
const LONG_HISTORY_LEN: usize = 600;
/// Zoom levels of the CPU history chart, in samples; the first is the default
/// and matches the scroll speed of the other live charts.
const HISTORY_WINDOWS: [usize; 3] = [120, 300, 600];
/// Processes recorded per history sample (the busiest ones, above MIN_CPU).
const HISTORY_PROCS_PER_SAMPLE: usize = 64;
const HISTORY_PROC_MIN_CPU: f64 = 0.1;

/// Busy processes captured for one sample of the CPU history.
struct ProcFrame {
    seq: u64,
    ts_ms: u64,
    procs: Vec<ProcSample>,
}

struct ProcSample {
    pid: u32,
    cpu: f32,
    rss: u64,
    name: Rc<str>, // shared between frames, interned per PID
}

fn read_cpufreq_meta() -> (f64, String, String) {
    let base_khz = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")
        .or_else(|_| fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/base_frequency"))
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(4000000.0);
    let drv = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_driver")
        .unwrap_or_else(|_| String::from("acpi-cpufreq"))
        .trim()
        .to_string();
    let gov = fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .unwrap_or_else(|_| String::from("schedutil"))
        .trim()
        .to_string();
    (base_khz / 1_000_000.0, drv, gov)
}

impl GuiController {
    /// Locks the shared sampler. Never hold two guards in one statement: the
    /// mutex is not re-entrant.
    fn sampler(&self) -> MutexGuard<'_, LinuxSystemSampler> {
        self.sampler.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn new(settings: AppSettings) -> Self {
        let mut sampler = LinuxSystemSampler::new();
        let _ = sampler.sample_tick();
        std::thread::sleep(Duration::from_millis(60));
        let snap = sampler.sample_tick();
        let n_cores = snap.cpu.logical_cores.max(1);
        let (cpu_base_ghz, cpu_driver, cpu_governor) = read_cpufreq_meta();

        let mut ctrl = Self {
            sampler: Arc::new(Mutex::new(sampler)),
            last_snap: snap,
            settings,
            cpu_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            cpu_kern_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            cpu_temp_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            mem_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            swap_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            disk_r_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            disk_w_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            net_rx_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            net_tx_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            gpu_busy_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            gpu_vram_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            power_hist: VecDeque::from(vec![0.0; HISTORY_LEN]),
            core_total_hist: (0..n_cores)
                .map(|_| VecDeque::from(vec![0.0; 60]))
                .collect(),
            core_kern_hist: (0..n_cores)
                .map(|_| VecDeque::from(vec![0.0; 60]))
                .collect(),
            cpu_base_ghz,
            cpu_driver,
            cpu_governor,
            cpu_long: VecDeque::from(vec![0.0; LONG_HISTORY_LEN]),
            cpu_kern_long: VecDeque::from(vec![0.0; LONG_HISTORY_LEN]),
            cpu_temp_long: VecDeque::from(vec![0.0; LONG_HISTORY_LEN]),
            proc_frames: VecDeque::with_capacity(LONG_HISTORY_LEN + 1),
            proc_names: HashMap::new(),
            selection: None,
            history_window: HISTORY_WINDOWS[0],
        };
        ctrl.push_history_from_last_snap();
        ctrl
    }

    fn push_history_from_last_snap(&mut self) {
        // Refresh CPU governor & driver in real-time
        let (_base, drv, gov) = read_cpufreq_meta();
        self.cpu_governor = gov;
        self.cpu_driver = drv;

        let snap = &self.last_snap;
        push_ring(&mut self.cpu_hist, snap.cpu.total_pct);
        push_ring(&mut self.cpu_kern_hist, snap.cpu.system_pct);
        let temp_pct = (snap.cpu.package_temp_c.unwrap_or(50.0) / 110.0 * 100.0).clamp(0.0, 100.0);
        push_ring(&mut self.cpu_temp_hist, temp_pct);

        push_ring(&mut self.mem_hist, snap.memory.used_pct);
        push_ring(&mut self.swap_hist, snap.memory.swap_pct);

        let disk_r_mb: f64 = snap
            .disks
            .iter()
            .map(|d| d.read_bytes_per_sec / (1024.0 * 1024.0))
            .sum();
        let disk_w_mb: f64 = snap
            .disks
            .iter()
            .map(|d| d.write_bytes_per_sec / (1024.0 * 1024.0))
            .sum();
        push_ring(&mut self.disk_r_hist, disk_r_mb);
        push_ring(&mut self.disk_w_hist, disk_w_mb);

        let net_rx_kb: f64 = snap
            .networks
            .iter()
            .map(|n| n.rx_bytes_per_sec / 1024.0)
            .sum();
        let net_tx_kb: f64 = snap
            .networks
            .iter()
            .map(|n| n.tx_bytes_per_sec / 1024.0)
            .sum();
        push_ring(&mut self.net_rx_hist, net_rx_kb);
        push_ring(&mut self.net_tx_hist, net_tx_kb);

        let gpu_busy = snap.gpus.first().map(|g| g.busy_pct).unwrap_or(0.0);
        let gpu_vram_pct = snap
            .gpus
            .first()
            .and_then(|g| {
                let u = g.vram_used_bytes? as f64;
                let t = g.vram_total_bytes?.max(1) as f64;
                Some((u / t * 100.0).clamp(0.0, 100.0))
            })
            .unwrap_or(0.0);
        push_ring(&mut self.gpu_busy_hist, gpu_busy);
        push_ring(&mut self.gpu_vram_hist, gpu_vram_pct);

        let pwr = snap
            .cpu
            .package_power_watts
            .or_else(|| snap.gpus.first().and_then(|g| g.power_watts))
            .unwrap_or(35.0);
        push_ring(&mut self.power_hist, (pwr / 200.0 * 100.0).clamp(0.0, 100.0));

        for (i, c) in snap.cpu.cores.iter().enumerate() {
            if i < self.core_total_hist.len() {
                push_ring_n(&mut self.core_total_hist[i], c.total_pct, 60);
                push_ring_n(&mut self.core_kern_hist[i], c.system_pct, 60);
            }
        }

        push_ring_n(&mut self.cpu_long, snap.cpu.total_pct, LONG_HISTORY_LEN);
        push_ring_n(&mut self.cpu_kern_long, snap.cpu.system_pct, LONG_HISTORY_LEN);
        push_ring_n(&mut self.cpu_temp_long, temp_pct, LONG_HISTORY_LEN);

        // snap.processes is sorted by CPU usage, busiest first.
        let mut procs = Vec::new();
        for p in snap
            .processes
            .iter()
            .take_while(|p| p.cpu_pct >= HISTORY_PROC_MIN_CPU)
            .take(HISTORY_PROCS_PER_SAMPLE)
        {
            let name = match self.proc_names.get(&p.pid) {
                Some(n) if **n == *p.name => Rc::clone(n),
                _ => {
                    let n: Rc<str> = Rc::from(p.name.as_str());
                    self.proc_names.insert(p.pid, Rc::clone(&n));
                    n
                }
            };
            procs.push(ProcSample { pid: p.pid, cpu: p.cpu_pct as f32, rss: p.mem_rss_bytes, name });
        }
        if self.proc_names.len() > 8192 {
            self.proc_names.clear(); // frames keep their own references
        }
        self.proc_frames.push_back(ProcFrame { seq: snap.seq, ts_ms: snap.timestamp_epoch_ms, procs });
        while self.proc_frames.len() > LONG_HISTORY_LEN {
            self.proc_frames.pop_front();
        }
        // Forget the selection once it has scrolled completely out of the window.
        if let Some((_, hi)) = self.selection
            && self.proc_frames.front().is_none_or(|f| hi < f.seq)
        {
            self.selection = None;
        }
    }

    /// Long-history ring index (0 = oldest point) of a sample sequence number.
    fn long_index_of(&self, seq: u64) -> f64 {
        (LONG_HISTORY_LEN - 1) as f64 - (self.last_snap.seq as f64 - seq as f64)
    }

    /// Index (0 = oldest visible point) of a sample within the zoomed chart window.
    fn window_index_of(&self, seq: u64) -> f64 {
        (self.history_window - 1) as f64 - (self.last_snap.seq as f64 - seq as f64)
    }

    /// Converts a drag over the history chart (screen fractions of its width) into
    /// a sample range. `phase` is the chart scroll phase the user was looking at.
    fn select_history_range(&mut self, f0: f32, f1: f32, phase: f32) {
        let n = self.history_window as f64;
        // Include exactly the samples whose points lie inside the dragged span.
        let i0 = (f0 as f64 * (n - 2.0) + phase as f64 - 1e-4).ceil();
        let i1 = (f1 as f64 * (n - 2.0) + phase as f64 + 1e-4).floor();
        let latest = self.last_snap.seq as f64;
        let to_seq = |i: f64| (latest - (n - 1.0 - i)).max(0.0) as u64;
        let oldest = self.proc_frames.front().map_or(0, |f| f.seq);
        let lo = to_seq(i0).max(oldest);
        let hi = to_seq(i1).min(self.last_snap.seq);
        self.selection = (hi >= lo).then_some((lo, hi));
    }

    /// Aggregates per-process activity over the selected range: rows sorted by
    /// average CPU, plus the number of samples and the average total CPU.
    fn history_range_rows(&self, lo: u64, hi: u64) -> (Vec<ProcRow>, usize, f64) {
        struct Agg {
            name: Rc<str>,
            cpu_sum: f64,
            peak: f32,
            rss_sum: u64,
            seen: u32,
        }
        let mut agg: HashMap<u32, Agg> = HashMap::new();
        let mut frames = 0usize;
        for f in self.proc_frames.iter().filter(|f| f.seq >= lo && f.seq <= hi) {
            frames += 1;
            for p in &f.procs {
                let e = agg.entry(p.pid).or_insert_with(|| Agg {
                    name: Rc::clone(&p.name),
                    cpu_sum: 0.0,
                    peak: 0.0,
                    rss_sum: 0,
                    seen: 0,
                });
                e.cpu_sum += p.cpu as f64;
                e.peak = e.peak.max(p.cpu);
                e.rss_sum += p.rss;
                e.seen += 1;
            }
        }
        let frames_f = frames.max(1) as f64;
        let mut list: Vec<(u32, Agg)> = agg.into_iter().collect();
        list.sort_by(|a, b| b.1.cpu_sum.total_cmp(&a.1.cpu_sum));
        let rows = list
            .into_iter()
            .take(18)
            .map(|(pid, a)| ProcRow {
                pid: pid as i32,
                name: SharedString::from(&*a.name),
                display_name: SharedString::from(&*a.name),
                cpu_str: SharedString::from(format!("{:.1}%", a.cpu_sum / frames_f)),
                gpu_str: SharedString::from(format!("{:.0}%", a.peak)),
                rss_str: SharedString::from(format_bytes(a.rss_sum / a.seen.max(1) as u64)),
                ..Default::default()
            })
            .collect();
        let i_lo = self.long_index_of(lo).max(0.0) as usize;
        let i_hi = (self.long_index_of(hi).max(0.0) as usize).min(LONG_HISTORY_LEN - 1);
        let avg_cpu = if i_hi >= i_lo {
            self.cpu_long.range(i_lo..=i_hi).sum::<f64>() / (i_hi - i_lo + 1) as f64
        } else {
            0.0
        };
        (rows, frames, avg_cpu)
    }

    fn frame_time_ms(&self, seq: u64) -> Option<u64> {
        let first = self.proc_frames.front()?.seq;
        self.proc_frames.get(seq.checked_sub(first)? as usize).map(|f| f.ts_ms)
    }

    fn tick(&mut self) {
        let snap = self.sampler().sample_tick();
        self.apply_snapshot(snap);
    }

    fn apply_snapshot(&mut self, snap: SystemSnapshot) {
        self.last_snap = snap;
        self.push_history_from_last_snap();
    }
}

type SnapshotHandler = Box<dyn FnMut(SystemSnapshot)>;

thread_local! {
    /// UI-thread handler for snapshots produced by the sampler thread.
    static SNAPSHOT_SINK: RefCell<Option<SnapshotHandler>> = const { RefCell::new(None) };
}

fn spawn_sampler_thread(sampler: Arc<Mutex<LinuxSystemSampler>>, period_ms: Arc<AtomicU64>) {
    let _ = std::thread::Builder::new()
        .name("pulsedeck-sampler".into())
        .spawn(move || {
            // Fixed-rate schedule so samples (and chart scrolling) stay evenly spaced.
            let mut next = Instant::now();
            loop {
                next += Duration::from_millis(period_ms.load(Ordering::Relaxed));
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else {
                    next = now;
                }
                let snap = sampler.lock().unwrap_or_else(|e| e.into_inner()).sample_tick();
                let delivered = slint::invoke_from_event_loop(move || {
                    SNAPSHOT_SINK.with(|sink| {
                        if let Some(handler) = sink.borrow_mut().as_mut() {
                            handler(snap);
                        }
                    });
                });
                if delivered.is_err() {
                    break; // event loop has quit
                }
            }
        });
}

/// Drives the sub-sample scroll offset of all live charts.
///
/// Charts are redrawn from new path data once per sample; in between, a
/// 30/60 Hz timer only moves a single `phase` value (0..1) that translates the
/// already-tessellated paths. The timer stops as soon as the scroll reaches the
/// newest sample or no chart is on screen, so an idle or hidden window costs nothing.
struct ChartScroll {
    win: slint::Weak<MainWindow>,
    timer: slint::Timer,
    state: RefCell<(Instant, u64)>, // (time of last sample, sample period ms)
}

impl ChartScroll {
    fn new(win: slint::Weak<MainWindow>) -> Self {
        Self {
            win,
            timer: slint::Timer::default(),
            state: RefCell::new((Instant::now(), 1000)),
        }
    }

    fn on_new_sample(self: &Rc<Self>, period_ms: u64) {
        *self.state.borrow_mut() = (Instant::now(), period_ms.max(1));
        let Some(w) = self.win.upgrade() else { return };
        let fps = w.get_chart_fps();
        let charts_visible = matches!(w.get_active_tab(), 0 | 1) && fps > 0;
        if !charts_visible {
            w.global::<ChartClock>().set_phase(1.0);
            self.timer.stop();
            return;
        }
        w.global::<ChartClock>().set_phase(0.0);
        let interval = Duration::from_micros(1_000_000 / fps as u64);
        if !self.timer.running() || self.timer.interval() != interval {
            let this = Rc::downgrade(self);
            self.timer.start(slint::TimerMode::Repeated, interval, move || {
                if let Some(this) = this.upgrade() {
                    this.frame();
                }
            });
        }
    }

    fn frame(&self) {
        let Some(w) = self.win.upgrade() else { return };
        let (at, period) = *self.state.borrow();
        let phase = (at.elapsed().as_secs_f32() * 1000.0 / period as f32).min(1.0);
        w.global::<ChartClock>().set_phase(phase);
        if phase >= 1.0 || !matches!(w.get_active_tab(), 0 | 1) {
            self.timer.stop();
        }
    }
}

fn push_ring(q: &mut VecDeque<f64>, val: f64) {
    push_ring_n(q, val, HISTORY_LEN);
}

fn push_ring_n(q: &mut VecDeque<f64>, val: f64, cap: usize) {
    if q.len() >= cap {
        q.pop_front();
    }
    q.push_back(val);
}

pub fn run_slint_gui(initial_settings: AppSettings) -> Result<()> {
    let win = MainWindow::new().map_err(|e| anyhow::anyhow!("Slint window init failed: {}", e))?;
    let ctrl = Rc::new(RefCell::new(GuiController::new(initial_settings)));

    win.set_sidebar_logo_img(render_sidebar_logo());
    win.global::<Theme>().set_lcd_ghost(render_lcd_ghost_cells());

    {
        let c = ctrl.borrow();
        apply_settings_to_window(&win, &c.settings);
        win.set_disk_scan_path(SharedString::from(c.sampler().disk_space.root_path.clone()));
        sync_all_models(&win, &c);
    }

    // Callback: Tab Selection
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_select_tab(move |tab_idx, title| {
            if let Some(w) = win_weak.upgrade() {
                w.set_active_tab(tab_idx);
                w.set_page_title(title);
                let c = ctrl_c.borrow();
                sync_filtered_views(&w, &c);
            }
        });
    }

    // Callback: Performance Sub-Item Selection
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_select_perf_item(move |idx| {
            if let Some(w) = win_weak.upgrade() {
                w.set_perf_selected_idx(idx);
                let c = ctrl_c.borrow();
                sync_performance_detail(&w, &c);
            }
        });
    }

    // Callback: Manual Refresh
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_refresh_now(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.tick();
                sync_all_models(&w, &c);
            }
        });
    }

    // Callbacks: CPU history range selection
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_history_range_selected(move |f0, f1| {
            if let Some(w) = win_weak.upgrade() {
                let phase = w.global::<ChartClock>().get_phase();
                let mut c = ctrl_c.borrow_mut();
                c.select_history_range(f0, f1, phase);
                sync_history_selection(&w, &c);
            }
        });
    }
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_history_range_cleared(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.selection = None;
                sync_history_selection(&w, &c);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_set_history_window(move |samples| {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.history_window = HISTORY_WINDOWS
                    .into_iter()
                    .find(|&n| n == samples as usize)
                    .unwrap_or(HISTORY_WINDOWS[0]);
                sync_all_models(&w, &c);
            }
        });
    }

    // Callback: Filter/Mode changes
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_refresh_filters(move || {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow();
                sync_filtered_views(&w, &c);
            }
        });
    }

    // Callback: Inspect Process
    {
        let win_weak = win.as_weak();
        win.on_inspect_process(move |pid| {
            if let Some(w) = win_weak.upgrade() {
                match inspect_process(pid as u32) {
                    Ok(info) => {
                        w.set_inspector_title(SharedString::from(format!(
                            "PROCESS INSPECTOR — PID {} ({}) | User: {} (UID {}) | State: {} | Threads: {} | Nice: {} | Affinity: {}",
                            info.pid, info.name, info.user, info.uid, info.state, info.threads, info.nice, info.cpu_affinity_hex
                        )));
                        let mut body = String::new();
                        body.push_str(&format!("Executable : {}\n", info.exe_path));
                        body.push_str(&format!("Working Dir: {}\n", info.cwd));
                        body.push_str(&format!("Cmdline    : {}\n", info.cmdline));
                        body.push_str(&format!(
                            "Memory     : RSS {} | VMS {} | Shared {} | Memory Maps: {}\n",
                            format_bytes(info.rss_bytes),
                            format_bytes(info.vms_bytes),
                            format_bytes(info.shared_bytes),
                            info.memory_maps_count
                        ));
                        body.push_str(&format!(
                            "Disk I/O   : Read {} | Write {} | Ctxt Switches: {} vol / {} nonvol\n\n",
                            format_bytes(info.read_bytes),
                            format_bytes(info.write_bytes),
                            info.voluntary_ctxt_switches,
                            info.nonvoluntary_ctxt_switches
                        ));
                        body.push_str("Open File Descriptors (Top 16):\n");
                        for fd in info.open_fds.iter().take(16) {
                            body.push_str(&format!("  fd {:<4} -> {}\n", fd.fd, fd.target));
                        }
                        if !info.top_mapped_files.is_empty() {
                            body.push_str("\nMapped Shared Libraries / Files:\n");
                            for mf in info.top_mapped_files.iter().take(12) {
                                body.push_str(&format!("  {}\n", mf));
                            }
                        }
                        w.set_inspector_body(SharedString::from(body));
                    }
                    Err(e) => {
                        w.set_inspector_title(SharedString::from(format!(
                            "Inspector error for PID {}: {}",
                            pid, e
                        )));
                    }
                }
            }
        });
    }

    // Callback: Signal / Priority Control
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_signal_process(move |pid, action_str| {
            if let Some(w) = win_weak.upgrade() {
                let act = match action_str.as_str() {
                    "TERM" => ProcessAction::Terminate,
                    "KILL" => ProcessAction::Kill,
                    "STOP" => ProcessAction::Stop,
                    "CONT" => ProcessAction::Continue,
                    "NICE_DEC" => {
                        let cur_nice = ctrl_c
                            .borrow()
                            .last_snap
                            .processes
                            .iter()
                            .find(|p| p.pid == pid as u32)
                            .map(|p| p.nice)
                            .unwrap_or(0);
                        ProcessAction::SetNice(cur_nice - 1)
                    }
                    "NICE_INC" => {
                        let cur_nice = ctrl_c
                            .borrow()
                            .last_snap
                            .processes
                            .iter()
                            .find(|p| p.pid == pid as u32)
                            .map(|p| p.nice)
                            .unwrap_or(0);
                        ProcessAction::SetNice(cur_nice + 1)
                    }
                    _ => ProcessAction::Terminate,
                };
                match control_process(pid as u32, act) {
                    Ok(msg) => w.set_status_message(SharedString::from(msg)),
                    Err(e) => w.set_status_message(SharedString::from(format!("Error: {}", e))),
                }
            }
        });
    }

    // Callback: Scan Disk Space
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_scan_disk_space(move |path_str| {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow_mut();
                let target = if path_str.trim().is_empty() {
                    "."
                } else {
                    path_str.trim()
                };
                let msg = {
                    let mut smp = c.sampler();
                    let ds = smp.rescan_disk_space(target);
                    format!(
                        "Scanned {} ({} files in {} ms)",
                        ds.root_path, ds.scanned_files, ds.scan_duration_ms
                    )
                };
                sync_disk_space_models(&w, &c);
                w.set_status_message(SharedString::from(msg));
            }
        });
    }

    // Callback: Flight Recorder Scrub
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_scrub_flight(move |target_idx| {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow();
                let len = c.sampler().flight_recorder.trace.samples.len() as i32;
                if len > 0 {
                    let clamped = target_idx.clamp(0, len - 1);
                    w.set_flight_scrub_index(clamped);
                    sync_flight_view(&w, &c);
                }
            }
        });
    }

    // Callback: Export Flight Recorder .pulsetrace
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_export_flight_trace(move || {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow();
                let json = serialize_flight_trace_json(c.sampler().export_flight_trace());
                let path = "pulsedeck-capture.pulsetrace";
                match fs::write(path, &json) {
                    Ok(_) => w.set_status_message(SharedString::from(format!(
                        "Exported {} frames to {}",
                        c.sampler().flight_recorder.trace.samples.len(),
                        path
                    ))),
                    Err(e) => w.set_status_message(SharedString::from(format!(
                        "Export failed: {}",
                        e
                    ))),
                }
            }
        });
    }

    // Callback: Run Benchmarks
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_run_benchmarks(move || {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow_mut();
                let cores = c.last_snap.cpu.logical_cores;
                let rep = c.sampler().run_all_benchmarks(cores).clone();
                w.set_bench_composite(SharedString::from(rep.composite_score.to_string()));
                w.set_bench_sc(SharedString::from(
                    rep.cpu_single_core.normalized_score.to_string(),
                ));
                w.set_bench_sc_sub(SharedString::from(format!(
                    "{:.1} {} ({} ms)",
                    rep.cpu_single_core.metric_value,
                    rep.cpu_single_core.metric_unit,
                    rep.cpu_single_core.duration_ms
                )));
                w.set_bench_mc(SharedString::from(
                    rep.cpu_multi_core.normalized_score.to_string(),
                ));
                w.set_bench_mc_sub(SharedString::from(format!(
                    "{:.1} {} ({} ms)",
                    rep.cpu_multi_core.metric_value,
                    rep.cpu_multi_core.metric_unit,
                    rep.cpu_multi_core.duration_ms
                )));
                w.set_bench_mem(SharedString::from(
                    rep.memory_bandwidth.normalized_score.to_string(),
                ));
                w.set_bench_mem_sub(SharedString::from(format!(
                    "RAM {:.1} GB/s",
                    rep.memory_bandwidth.metric_value
                )));
                w.set_bench_disk(SharedString::from(
                    rep.disk_throughput.normalized_score.to_string(),
                ));
                w.set_bench_disk_sub(SharedString::from(format!(
                    "Disk {:.0} MB/s",
                    rep.disk_throughput.metric_value
                )));
                w.set_status_message(SharedString::from(format!(
                    "Composite PulseDeck Score: {}",
                    rep.composite_score
                )));
            }
        });
    }

    // Callback: Export System Info
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_export_sysinfo(move || {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow();
                if let Some(ref info) = c.sampler().system_info {
                    let text = format_system_info_report(info);
                    let path = "pulsedeck-system-info.txt";
                    match fs::write(path, &text) {
                        Ok(_) => w.set_status_message(SharedString::from(format!(
                            "Exported System Info report to {}",
                            path
                        ))),
                        Err(e) => w.set_status_message(SharedString::from(format!(
                            "Export failed: {}",
                            e
                        ))),
                    }
                }
            }
        });
    }

    // Customisation Callbacks
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_apply_theme_preset(move |preset_name| {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.settings.theme = ThemeColors::from_preset(preset_name.as_str());
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
                sync_all_models(&w, &c);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_apply_custom_colors(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                if let Some(rgb) = parse_hex_color(w.get_custom_primary_hex().as_str()) {
                    c.settings.theme.accent_primary = rgb;
                }
                if let Some(rgb) = parse_hex_color(w.get_custom_secondary_hex().as_str()) {
                    c.settings.theme.accent_secondary = rgb;
                }
                if let Some(rgb) = parse_hex_color(w.get_custom_bg_hex().as_str()) {
                    c.settings.theme.bg_main = rgb;
                }
                if let Some(rgb) = parse_hex_color(w.get_custom_card_hex().as_str()) {
                    c.settings.theme.bg_card = rgb;
                }
                if let Some(rgb) = parse_hex_color(w.get_custom_text_hex().as_str()) {
                    c.settings.theme.text_main = rgb;
                }
                c.settings.theme.preset_name = String::from("Custom");
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
                sync_all_models(&w, &c);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_select_font_family(move |fam| {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                if !fam.trim().is_empty() {
                    c.settings.font_family = fam.trim().to_string();
                    let _ = c.settings.save();
                    apply_settings_to_window(&w, &c.settings);
                }
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_adjust_font_size(move |delta| {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                let next = (c.settings.base_font_size as i32 + delta).clamp(9, 22) as u32;
                c.settings.base_font_size = next;
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_toggle_compact_rows(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.settings.compact_rows = !c.settings.compact_rows;
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_cycle_chart_fps(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.settings.chart_fps = match c.settings.chart_fps {
                    60 => 30,
                    30 => 0,
                    _ => 60,
                };
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_toggle_graph_grid(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.settings.show_graph_grid = !c.settings.show_graph_grid;
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
                sync_all_models(&w, &c);
            }
        });
    }

    // Telemetry sampling takes 50-70 ms, so it runs on a worker thread; the UI
    // thread only receives finished snapshots and stays free to render frames.
    let period_ms = Arc::new(AtomicU64::new(ctrl.borrow().settings.refresh_ms));
    let scroll = Rc::new(ChartScroll::new(win.as_weak()));
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        let scroll_c = Rc::clone(&scroll);
        let period_c = Arc::clone(&period_ms);
        SNAPSHOT_SINK.with(|sink| {
            *sink.borrow_mut() = Some(Box::new(move |snap: SystemSnapshot| {
                if let Some(w) = win_weak.upgrade() {
                    let mut c = ctrl_c.borrow_mut();
                    c.apply_snapshot(snap);
                    sync_all_models(&w, &c);
                    drop(c);
                    scroll_c.on_new_sample(period_c.load(Ordering::Relaxed));
                }
            }));
        });
        spawn_sampler_thread(Arc::clone(&ctrl.borrow().sampler), Arc::clone(&period_ms));
    }
    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        let period_c = Arc::clone(&period_ms);
        win.on_set_refresh_rate(move |ms| {
            if let Some(w) = win_weak.upgrade() {
                let clamped = (ms as u64).clamp(200, 5000);
                let mut c = ctrl_c.borrow_mut();
                c.settings.refresh_ms = clamped;
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
                period_c.store(clamped, Ordering::Relaxed);
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_save_settings(move || {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow();
                let _ = c.settings.save();
                w.set_status_message(SharedString::from("Settings saved"));
            }
        });
    }

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_reset_settings(move || {
            if let Some(w) = win_weak.upgrade() {
                let mut c = ctrl_c.borrow_mut();
                c.settings = AppSettings::default();
                let _ = c.settings.save();
                apply_settings_to_window(&w, &c.settings);
                sync_all_models(&w, &c);
            }
        });
    }

    win.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });

    {
        let win_weak = win.as_weak();
        let ctrl_c = Rc::clone(&ctrl);
        win.on_toggle_flight_recording(move |rec| {
            if let Some(w) = win_weak.upgrade() {
                let c = ctrl_c.borrow_mut();
                c.sampler().flight_recorder.recording = rec;
                if rec {
                    w.set_status_message(SharedString::from("Flight Recorder active"));
                } else {
                    let json = serialize_flight_trace_json(c.sampler().export_flight_trace());
                    let path = "pulsedeck-capture.pulsetrace";
                    let _ = fs::write(path, &json);
                    w.set_status_message(SharedString::from(format!(
                        "Recording paused & saved ({} frames) to {}",
                        c.sampler().flight_recorder.trace.samples.len(),
                        path
                    )));
                }
            }
        });
    }

    win.on_copy_to_clipboard(|text| {
        copy_text_to_clipboard(text.as_str());
    });

    {
        let win_weak = win.as_weak();
        win.on_relaunch_as_root(move || {
            if let Some(w) = win_weak.upgrade() {
                match relaunch_as_root() {
                    Ok(()) => {}
                    Err(e) => {
                        w.set_status_message(SharedString::from(format!("Elevate error: {}", e)));
                    }
                }
            }
        });
    }

    {
        let win_weak = win.as_weak();
        win.on_toggle_windows_default_taskmgr(move |enable| {
            if let Some(w) = win_weak.upgrade() {
                match toggle_windows_default_taskmgr(enable) {
                    Ok(msg) => {
                        w.set_status_message(SharedString::from(msg));
                    }
                    Err(e) => {
                        w.set_status_message(SharedString::from(format!("TaskMgr error: {}", e)));
                    }
                }
            }
        });
    }

    win.set_is_root_user(is_elevated_root());

    win.run().map_err(|e| anyhow::anyhow!("Slint event loop error: {}", e))
}

fn is_elevated_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn relaunch_as_root() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let exe = std::env::current_exe()?;
        let args: Vec<String> = std::env::args().skip(1).collect();

        let pkexec_exists = std::process::Command::new("which")
            .arg("pkexec")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let mut cmd = if pkexec_exists {
            let mut c = std::process::Command::new("pkexec");
            if let Ok(d) = std::env::var("DISPLAY") {
                c.env("DISPLAY", d);
            }
            if let Ok(w) = std::env::var("WAYLAND_DISPLAY") {
                c.env("WAYLAND_DISPLAY", w);
            }
            if let Ok(x) = std::env::var("XAUTHORITY") {
                c.env("XAUTHORITY", x);
            }
            c.arg(&exe);
            c.args(&args);
            c
        } else {
            let mut c = std::process::Command::new("sudo");
            c.arg("-E");
            c.arg(&exe);
            c.args(&args);
            c
        };

        cmd.spawn()?;
        std::process::exit(0);
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("Relaunch as root is only supported on Linux/Unix");
    }
}

fn toggle_windows_default_taskmgr(enable: bool) -> anyhow::Result<String> {
    #[cfg(target_os = "windows")]
    {
        let key = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe";
        if enable {
            let exe = std::env::current_exe()?;
            let exe_str = exe.to_string_lossy();
            let status = std::process::Command::new("reg")
                .args(["add", key, "/v", "Debugger", "/t", "REG_SZ", "/d", &exe_str, "/f"])
                .status()?;
            if status.success() {
                Ok("PulseDeck set as default Windows Task Manager.".to_string())
            } else {
                anyhow::bail!("Registry write failed. Please run PulseDeck as Administrator.")
            }
        } else {
            let status = std::process::Command::new("reg")
                .args(["delete", key, "/v", "Debugger", "/f"])
                .status()?;
            if status.success() {
                Ok("Restored standard Windows Task Manager.".to_string())
            } else {
                anyhow::bail!("Failed to remove Windows registry entry.")
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = enable;
        anyhow::bail!("Default Task Manager replacement is a Windows-only feature.");
    }
}

fn copy_text_to_clipboard(text: &str) {
    #[cfg(unix)]
    {
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            use std::io::Write;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
            return;
        }
    }
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;
        if let Ok(mut child) = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
    }
}

fn sync_all_models(w: &MainWindow, c: &GuiController) {
    let snap = &c.last_snap;

    // 1. Bottom Status Bar
    w.set_status_left_text(SharedString::from(format!(
        "Native providers healthy · {} processes",
        snap.processes.len()
    )));
    let rec_frames = c.sampler().flight_recorder.trace.samples.len() as u64;
    let rec_secs = (rec_frames * c.settings.refresh_ms) / 1000;
    let rec_cs = ((rec_frames * c.settings.refresh_ms) % 1000) / 10;
    w.set_status_timer_text(SharedString::from(format!(
        "{:02}:{:02}:{:02}.{:02}",
        rec_secs / 3600,
        (rec_secs % 3600) / 60,
        rec_secs % 60,
        rec_cs
    )));
    let primary_ip = snap
        .networks
        .iter()
        .find(|n| !n.ipv4.is_empty() && n.ipv4 != "127.0.0.1")
        .map(|n| n.ipv4.as_str())
        .unwrap_or("127.0.0.1");
    w.set_status_ip_text(SharedString::from(format!("IP {}", primary_ip)));
    w.set_status_gen_text(SharedString::from(format!("Generation {}", snap.seq)));

    // 2. Summary Page Top Row: Card 1 (SYS | 4.0GHZ Vertical Segmented LED Bars)
    let ghz = snap.cpu.avg_freq_mhz / 1000.0;
    w.set_sum_vfd_clock(SharedString::from(format!("{:.1}GHZ", ghz)));
    w.set_sum_vfd_cpu_pct(SharedString::from(format!("{:.0}%", snap.cpu.total_pct)));

    let cpu_ratio = (snap.cpu.total_pct / 100.0).clamp(0.0, 1.0);
    let max_ghz = c.cpu_base_ghz.max(4.5);
    let clock_ratio = (ghz / max_ghz).clamp(0.0, 1.0);
    let temp_c = snap.cpu.package_temp_c.unwrap_or(52.0);
    let temp_ratio = (temp_c / 100.0).clamp(0.0, 1.0);
    let gpu_busy = snap.gpus.first().map(|g| g.busy_pct).unwrap_or(0.0);
    let gpu_ratio = (gpu_busy / 100.0).clamp(0.0, 1.0);

    w.set_sum_vertical_bars_img(render_4_vertical_led_bars(
        cpu_ratio,
        clock_ratio,
        temp_ratio,
        gpu_ratio,
    ));
    w.set_sum_bar_cpu_val(SharedString::from(format!("{:.0}%", snap.cpu.total_pct)));
    w.set_sum_bar_clock_val(SharedString::from(format!("{:.1}G", ghz)));
    w.set_sum_bar_temp_val(SharedString::from(format!("{:.0}°", temp_c)));
    w.set_sum_bar_gpu_val(SharedString::from(format!("{:.0}%", gpu_busy)));

    // Summary Card 2: CPU HISTORY (Utilization green, Temp orange, Kernel purple)
    let tail = |q: &VecDeque<f64>| -> VecDeque<f64> {
        q.iter().skip(q.len().saturating_sub(c.history_window)).copied().collect()
    };
    let (cpu_stroke, _) = generate_svg_stroke_path(&tail(&c.cpu_long), 100.0);
    let cpu_fill = generate_svg_fill_path(&cpu_stroke);
    let (temp_stroke, _) = generate_svg_stroke_path(&tail(&c.cpu_temp_long), 100.0);
    let (kern_stroke, _) = generate_svg_stroke_path(&tail(&c.cpu_kern_long), 100.0);
    w.set_sum_cpu_vector_stroke(SharedString::from(cpu_stroke));
    w.set_sum_cpu_vector_fill(SharedString::from(cpu_fill));
    w.set_sum_cpu_vector_temp(SharedString::from(temp_stroke));
    w.set_sum_cpu_vector_kernel(SharedString::from(kern_stroke));
    w.set_sum_cpu_points(c.history_window as i32);
    w.set_sum_cpu_window_label(SharedString::from(format_span_secs(
        (c.history_window as u64 * c.settings.refresh_ms) / 1000,
    )));

    w.set_sum_cpu_logical_str(SharedString::from(format!(
        "{} logical processors",
        snap.cpu.logical_cores
    )));
    w.set_sum_cpu_speed_str(SharedString::from(format!("Speed {:.2} GHz", ghz)));

    // Summary Card 3: live TOP 18 CPU PROCESSES, or activity in the selected history range
    sync_history_selection(w, c);

    // Summary Middle Row: Memory Utilization
    let mem_ratio = (snap.memory.used_pct / 100.0).clamp(0.0, 1.0);
    w.set_sum_mem_vert_bar_img(render_single_vertical_led_bar(mem_ratio, [184, 77, 255]));
    w.set_sum_mem_pct_val(SharedString::from(format!("{:.0}%", snap.memory.used_pct)));
    w.set_sum_mem_header_right(SharedString::from(format!(
        "{} / {}",
        format_bytes(snap.memory.used_bytes),
        format_bytes(snap.memory.total_bytes)
    )));
    let (mem_stroke, _) = generate_svg_stroke_path(&c.mem_hist, 100.0);
    let mem_fill = generate_svg_fill_path(&mem_stroke);
    w.set_sum_mem_vector_stroke(SharedString::from(mem_stroke));
    w.set_sum_mem_vector_fill(SharedString::from(mem_fill));
    w.set_sum_mem_avail_str(SharedString::from(format!(
        "Available {}",
        format_bytes(snap.memory.available_bytes)
    )));
    w.set_sum_mem_cached_str(SharedString::from(format!(
        "Cached {}",
        format_bytes(snap.memory.cached_bytes)
    )));
    w.set_sum_mem_swap_str(SharedString::from(format!(
        "Swap {}",
        format_bytes(snap.memory.swap_used_bytes)
    )));

    // Summary Bottom Row: 4 Horizontal Segmented LED Cards
    let p_ratio = (snap.pressure.score_percent / 100.0).clamp(0.0, 1.0);
    w.set_sum_card1_right(SharedString::from(snap.pressure.level.as_str()));
    w.set_sum_card1_color(Color::from_rgb_u8(34, 229, 139));
    w.set_sum_card1_sub(SharedString::from(format!(
        "Load demand {:.1}% · Memory stalls {:.1}%",
        snap.pressure.load_per_core * 100.0,
        snap.pressure.mem_psi_some_avg10
    )));
    w.set_sum_card1_bar(render_horizontal_led_bar(
        260,
        18,
        24,
        p_ratio,
        [34, 229, 139],
    ));

    let primary_net = snap
        .networks
        .iter()
        .find(|n| n.name != "lo")
        .or_else(|| snap.networks.first());
    if let Some(net) = primary_net {
        let total_bps = net.rx_bytes_per_sec + net.tx_bytes_per_sec;
        let scale_bps = 33.3 * 1024.0 * 1024.0;
        let net_ratio = (total_bps / scale_bps).clamp(0.04, 1.0);
        let speed_str = net
            .speed_mbps
            .map(|mb| {
                if mb >= 1000 {
                    format!("{} GbE", mb / 1000)
                } else {
                    format!("{} MbE", mb)
                }
            })
            .unwrap_or_else(|| String::from("1 GbE"));
        w.set_sum_card2_title(SharedString::from(net.name.as_str()));
        w.set_sum_card2_right(SharedString::from(format!(
            "{}/sec",
            format_bytes(total_bps as u64)
        )));
        w.set_sum_card2_sub_left(SharedString::from(format!(
            "{} · {:.1}%",
            speed_str,
            (total_bps / (125.0 * 1024.0 * 1024.0) * 100.0).clamp(0.0, 100.0)
        )));
        w.set_sum_card2_sub_right(SharedString::from("Scale 33.3 MB/sec"));
        w.set_sum_card2_bar(render_horizontal_led_bar(
            260,
            18,
            24,
            net_ratio,
            [25, 181, 254],
        ));
    }

    let disk_r: f64 = snap.disks.iter().map(|d| d.read_bytes_per_sec).sum();
    let disk_w: f64 = snap.disks.iter().map(|d| d.write_bytes_per_sec).sum();
    let disk_util = snap
        .disks
        .iter()
        .map(|d| d.busy_pct)
        .fold(0.0_f64, f64::max);
    w.set_sum_card3_right(SharedString::from(format!("{:.1}%", disk_util)));
    w.set_sum_card3_sub_left(SharedString::from(format!(
        "R {}/sec · W {}/sec",
        format_bytes(disk_r as u64),
        format_bytes(disk_w as u64)
    )));
    w.set_sum_card3_bar(render_horizontal_led_bar(
        260,
        18,
        24,
        (disk_util / 100.0).clamp(0.03, 1.0),
        [34, 229, 139],
    ));

    if let Some(gpu) = snap.gpus.first() {
        w.set_sum_card4_title(SharedString::from(gpu.name.as_str()));
        w.set_sum_card4_right(SharedString::from(
            gpu.temp_c
                .map_or(String::from("-- °C"), |t| format!("{:.0} °C", t)),
        ));
        w.set_sum_card4_sub(SharedString::from(format!(
            "GPU Power: {}",
            gpu.power_watts
                .map_or(String::from("-- W"), |p| format!("{:.1} W", p))
        )));
        w.set_sum_card4_bar(render_horizontal_led_bar(
            260,
            18,
            24,
            (gpu.busy_pct / 100.0).clamp(0.08, 1.0),
            [25, 181, 254],
        ));
    }

    // 3. Performance Left Navigation List + Right Detail Pane
    sync_performance_nav(w, c);
    sync_performance_detail(w, c);

    // 4. Sensors
    let mut s_rows = Vec::new();
    for grp in &snap.sensors {
        for r in &grp.readings {
            let ratio = if r.max > r.min {
                ((r.current - r.min) / (r.max - r.min)).clamp(0.0, 1.0) as f32
            } else {
                0.5
            };
            s_rows.push(SensorRow {
                chip: SharedString::from(grp.chip_name.as_str()),
                adapter: SharedString::from(grp.adapter.as_str()),
                label: SharedString::from(r.label.as_str()),
                category: SharedString::from(r.category.as_str()),
                cur_str: SharedString::from(format!("{:.1} {}", r.current, r.unit)),
                min_str: SharedString::from(format!("{:.1} {}", r.min, r.unit)),
                max_str: SharedString::from(format!("{:.1} {}", r.max, r.unit)),
                ratio,
            });
        }
    }
    w.set_sensors(ModelRc::new(VecModel::from(s_rows)));

    sync_disk_space_models(w, c);

    if let Some(ref info) = c.sampler().system_info {
        w.set_sysinfo_text(SharedString::from(format_system_info_report(info)));
    }

    let f_len = c.sampler().flight_recorder.trace.samples.len() as i32;
    let prev_count = w.get_flight_frame_count();
    let cur_scrub = w.get_flight_scrub_index();
    w.set_flight_frame_count(f_len);
    if cur_scrub >= prev_count - 1 || cur_scrub == 0 {
        w.set_flight_scrub_index((f_len - 1).max(0));
    }
    sync_flight_view(w, c);
    sync_filtered_views(w, c);
}

fn sync_performance_nav(w: &MainWindow, c: &GuiController) {
    let snap = &c.last_snap;
    let net = snap
        .networks
        .iter()
        .find(|n| n.name != "lo")
        .or_else(|| snap.networks.first());
    let net_name = net.map(|n| n.name.as_str()).unwrap_or("eth0");
    let net_sub2 = net
        .map(|n| {
            format!(
                "S: {}/sec R: {}/sec",
                format_bytes(n.tx_bytes_per_sec as u64),
                format_bytes(n.rx_bytes_per_sec as u64)
            )
        })
        .unwrap_or_else(|| String::from("S: 0 B/sec R: 0 B/sec"));

    let gpu = snap.gpus.first();
    let gpu_name = gpu.map(|g| g.name.as_str()).unwrap_or("GPU 0");
    let gpu_sub2 = gpu
        .map(|g| format!("{:.1}%", g.busy_pct))
        .unwrap_or_else(|| String::from("0.0%"));

    let disk_util = snap
        .disks
        .iter()
        .map(|d| d.busy_pct)
        .fold(0.0_f64, f64::max);

    let pwr_sub = snap
        .cpu
        .package_power_watts
        .or_else(|| gpu.and_then(|g| g.power_watts))
        .map(|p| format!("{:.1} W", p))
        .unwrap_or_else(|| String::from("Power telemetry active"));

    let therm_temp = snap
        .cpu
        .package_temp_c
        .or_else(|| gpu.and_then(|g| g.temp_c))
        .unwrap_or(55.0);

    let items = vec![
        PerfNavItem {
            index: 0,
            icon: SharedString::from("⚙"),
            icon_color: Color::from_rgb_u8(54, 197, 255),
            title: SharedString::from("CPU"),
            sub1: SharedString::from(format!("{:.1}%", snap.cpu.total_pct)),
            sub2: SharedString::from(format!("{} logical processors", snap.cpu.logical_cores)),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.cpu_hist,
                100.0,
                [54, 197, 255],
                [14, 28, 40],
                true,
            ),
        },
        PerfNavItem {
            index: 1,
            icon: SharedString::from("▦"),
            icon_color: Color::from_rgb_u8(184, 77, 255),
            title: SharedString::from("Memory"),
            sub1: SharedString::from(format!(
                "{} / {}",
                format_bytes(snap.memory.used_bytes),
                format_bytes(snap.memory.total_bytes)
            )),
            sub2: SharedString::from(format!("{:.1}%", snap.memory.used_pct)),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.mem_hist,
                100.0,
                [184, 77, 255],
                [28, 16, 40],
                true,
            ),
        },
        PerfNavItem {
            index: 2,
            icon: SharedString::from("🖴"),
            icon_color: Color::from_rgb_u8(34, 229, 139),
            title: SharedString::from("Disk"),
            sub1: SharedString::from("Physical disk"),
            sub2: SharedString::from(format!("{:.1}%", disk_util)),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.disk_w_hist,
                0.0,
                [34, 229, 139],
                [12, 32, 22],
                true,
            ),
        },
        PerfNavItem {
            index: 3,
            icon: SharedString::from("🌐"),
            icon_color: Color::from_rgb_u8(25, 181, 254),
            title: SharedString::from(net_name),
            sub1: SharedString::from("Network adapter"),
            sub2: SharedString::from(net_sub2),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.net_rx_hist,
                0.0,
                [25, 181, 254],
                [12, 26, 40],
                true,
            ),
        },
        PerfNavItem {
            index: 4,
            icon: SharedString::from("🖥"),
            icon_color: Color::from_rgb_u8(25, 181, 254),
            title: SharedString::from(gpu_name),
            sub1: SharedString::from("GPU device"),
            sub2: SharedString::from(gpu_sub2),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.gpu_busy_hist,
                100.0,
                [25, 181, 254],
                [12, 26, 40],
                true,
            ),
        },
        PerfNavItem {
            index: 5,
            icon: SharedString::from("⚡"),
            icon_color: Color::from_rgb_u8(245, 200, 35),
            title: SharedString::from("Energy"),
            sub1: SharedString::from("Whole machine"),
            sub2: SharedString::from(pwr_sub),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.power_hist,
                100.0,
                [245, 200, 35],
                [36, 30, 12],
                true,
            ),
        },
        PerfNavItem {
            index: 6,
            icon: SharedString::from("🌡"),
            icon_color: Color::from_rgb_u8(245, 150, 35),
            title: SharedString::from("Thermals"),
            sub1: SharedString::from("Whole machine"),
            sub2: SharedString::from(format!("{:.0} °C", therm_temp)),
            sparkline: render_single_area_graph(
                64,
                42,
                &c.cpu_temp_hist,
                100.0,
                [245, 150, 35],
                [38, 24, 12],
                true,
            ),
        },
    ];
    w.set_perf_nav_items(ModelRc::new(VecModel::from(items)));
}

fn sync_performance_detail(w: &MainWindow, c: &GuiController) {
    let snap = &c.last_snap;
    let sel = w.get_perf_selected_idx();

    match sel {
        0 => {
            // CPU Detail View (Screenshot 3)
            w.set_perf_header_title(SharedString::from("CPU"));
            w.set_perf_header_pct(SharedString::from(format!("{:.1}%", snap.cpu.total_pct)));
            w.set_perf_header_sub(SharedString::from(format!(
                "{} logical processors",
                snap.cpu.logical_cores
            )));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (snap.cpu.total_pct / 100.0).clamp(0.0, 1.0),
                [34, 229, 139],
            ));

            if w.get_perf_cpu_expanded() {
                let n_cores = c.core_total_hist.len().max(1);
                let cols = if n_cores >= 16 {
                    8i32
                } else if n_cores >= 8 {
                    4i32
                } else {
                    n_cores.min(4) as i32
                };
                let rows = ((n_cores as i32) + cols - 1) / cols;
                w.set_perf_core_cols(cols);
                w.set_perf_core_rows(rows);

                let mut items = Vec::with_capacity(n_cores);
                for i in 0..n_cores {
                    let tot_hist = &c.core_total_hist[i];
                    let (stroke, _) = generate_svg_stroke_path(tot_hist, 100.0);
                    let fill = generate_svg_fill_path(&stroke);
                    let kern_stroke = if let Some(kh) = c.core_kern_hist.get(i) {
                        generate_svg_stroke_path(kh, 100.0).0
                    } else {
                        String::new()
                    };
                    let cur_pct = snap.cpu.cores.get(i).map(|co| co.total_pct).unwrap_or(0.0);

                    items.push(CorePlotData {
                        core_idx: i as i32,
                        label: SharedString::from(format!("CPU {}", i)),
                        stroke_path: SharedString::from(stroke),
                        fill_path: SharedString::from(fill),
                        kernel_path: SharedString::from(kern_stroke),
                        val_str: SharedString::from(format!("{:.0}%", cur_pct)),
                    });
                }
                w.set_perf_core_items(ModelRc::new(VecModel::from(items)));
            } else {
                w.set_perf_has_secondary_graph(false);
                w.set_perf_main_graph_label(SharedString::from("% Utilization"));
                let (stroke, _) = generate_svg_stroke_path(&c.cpu_hist, 100.0);
                let fill = generate_svg_fill_path(&stroke);
                let (kern_stroke, _) = generate_svg_stroke_path(&c.cpu_kern_hist, 100.0);
                w.set_perf_vector_stroke(SharedString::from(stroke));
                w.set_perf_vector_fill(SharedString::from(fill));
                w.set_perf_vector_kernel(SharedString::from(kern_stroke));
                w.set_perf_vector_has_kernel(true);
                w.set_perf_vector_color(Color::from_rgb_u8(54, 197, 255));
                w.set_perf_vector_fill_color(Color::from_argb_u8(40, 54, 197, 255));
                w.set_perf_vector_max_label(SharedString::from("100%"));
                w.set_perf_vector_min_label(SharedString::from("0%"));
                w.set_perf_vector_current_val(SharedString::from(format!("{:.1}%", snap.cpu.total_pct)));
            }

            let total_threads: u32 = snap.processes.iter().map(|p| p.threads).sum();
            w.set_perf_m1_label(SharedString::from("Utilization"));
            w.set_perf_m1_val(SharedString::from(format!("{:.1}%", snap.cpu.total_pct)));
            w.set_perf_m2_label(SharedString::from("Speed"));
            w.set_perf_m2_val(SharedString::from(format!(
                "{:.2} GHz",
                snap.cpu.avg_freq_mhz / 1000.0
            )));
            w.set_perf_m3_label(SharedString::from("Processes"));
            w.set_perf_m3_val(SharedString::from(snap.processes.len().to_string()));
            w.set_perf_m4_label(SharedString::from("Threads"));
            w.set_perf_m4_val(SharedString::from(total_threads.to_string()));
            w.set_perf_m5_label(SharedString::from("Up time"));
            w.set_perf_m5_val(SharedString::from(format_uptime_dhms(
                snap.cpu.uptime_seconds as u64,
            )));
            w.set_perf_m6_label(SharedString::from(""));
            w.set_perf_m6_val(SharedString::from(""));
            w.set_perf_m7_label(SharedString::from(""));
            w.set_perf_m7_val(SharedString::from(""));

            w.set_perf_kv1_k(SharedString::from("Base speed:"));
            w.set_perf_kv1_v(SharedString::from(format!("{:.2} GHz", c.cpu_base_ghz)));
            w.set_perf_kv2_k(SharedString::from("Physical cores:"));
            w.set_perf_kv2_v(SharedString::from(snap.cpu.physical_cores.to_string()));
            w.set_perf_kv3_k(SharedString::from("Virtualization:"));
            w.set_perf_kv3_v(SharedString::from("Hardware virtualization supported"));
            w.set_perf_kv4_k(SharedString::from("Frequency driver:"));
            w.set_perf_kv4_v(SharedString::from(c.cpu_driver.as_str()));
            w.set_perf_kv5_k(SharedString::from("Sockets:"));
            w.set_perf_kv5_v(SharedString::from("1"));
            w.set_perf_kv6_k(SharedString::from("Logical processors:"));
            w.set_perf_kv6_v(SharedString::from(snap.cpu.logical_cores.to_string()));
            w.set_perf_kv7_k(SharedString::from("Virtual machine:"));
            w.set_perf_kv7_v(SharedString::from("No"));
            w.set_perf_kv8_k(SharedString::from("Governor:"));
            w.set_perf_kv8_v(SharedString::from(c.cpu_governor.as_str()));
        }
        4 => {
            // GPU Detail View (Screenshot 2)
            let gpu = snap.gpus.first();
            let title = gpu.map(|g| g.name.as_str()).unwrap_or("GPU Device");
            let busy = gpu.map(|g| g.busy_pct).unwrap_or(0.0);
            let sub = gpu
                .map(|g| format!("{} · {}", g.pci_slot, g.driver))
                .unwrap_or_else(|| String::from("drm"));
            let vram_u = gpu.and_then(|g| g.vram_used_bytes).unwrap_or(0);
            let vram_t = gpu.and_then(|g| g.vram_total_bytes).unwrap_or(0);
            let gtt_u = gpu.and_then(|g| g.gtt_used_bytes).unwrap_or(0);

            w.set_perf_header_title(SharedString::from(title));
            w.set_perf_header_pct(SharedString::from(format!("{:.1}%", busy)));
            w.set_perf_header_sub(SharedString::from(sub));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (busy / 100.0).clamp(0.0, 1.0),
                [25, 181, 254],
            ));

            w.set_perf_main_graph_label(SharedString::from("% Utilization"));
            let (stroke, _) = generate_svg_stroke_path(&c.gpu_busy_hist, 100.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(54, 197, 255));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 54, 197, 255));
            w.set_perf_vector_max_label(SharedString::from("100%"));
            w.set_perf_vector_min_label(SharedString::from("0%"));
            w.set_perf_vector_current_val(SharedString::from(format!("{:.1}%", busy)));

            w.set_perf_has_secondary_graph(true);
            w.set_perf_sec_graph_label(SharedString::from("GPU memory in use"));
            w.set_perf_sec_graph_right(SharedString::from(format!(
                "{} / {}",
                format_bytes(vram_u),
                format_bytes(vram_t)
            )));
            let (sec_stroke, _) = generate_svg_stroke_path(&c.gpu_vram_hist, 100.0);
            let sec_fill = generate_svg_fill_path(&sec_stroke);
            w.set_perf_sec_vector_stroke(SharedString::from(sec_stroke));
            w.set_perf_sec_vector_fill(SharedString::from(sec_fill));
            w.set_perf_sec_vector_color(Color::from_rgb_u8(184, 77, 255));
            w.set_perf_sec_vector_fill_color(Color::from_argb_u8(40, 184, 77, 255));
            w.set_perf_sec_vector_max_label(SharedString::from(format_bytes(vram_t)));
            w.set_perf_sec_vector_min_label(SharedString::from("0 MB"));
            w.set_perf_sec_vector_current_val(SharedString::from(format_bytes(vram_u)));

            w.set_perf_m1_label(SharedString::from("Capacity"));
            w.set_perf_m1_val(SharedString::from(format_bytes(vram_t)));
            w.set_perf_m2_label(SharedString::from("Used"));
            w.set_perf_m2_val(SharedString::from(format_bytes(vram_u)));
            w.set_perf_m3_label(SharedString::from("Utilization"));
            w.set_perf_m3_val(SharedString::from(format!("{:.1}%", busy)));
            w.set_perf_m4_label(SharedString::from("Current clock"));
            w.set_perf_m4_val(SharedString::from(
                gpu.and_then(|g| g.clock_mhz)
                    .map_or(String::from("--"), |m| format!("{:.2} GHz", m / 1000.0)),
            ));
            w.set_perf_m5_label(SharedString::from("Power draw"));
            w.set_perf_m5_val(SharedString::from(
                gpu.and_then(|g| g.power_watts)
                    .map_or(String::from("-- W"), |p| format!("{:.1} W", p)),
            ));
            w.set_perf_m6_label(SharedString::from("Temperature"));
            w.set_perf_m6_val(SharedString::from(
                gpu.and_then(|g| g.temp_c)
                    .map_or(String::from("-- °C"), |t| format!("{:.0} °C", t)),
            ));
            w.set_perf_m7_label(SharedString::from("Fan speed"));
            w.set_perf_m7_val(SharedString::from(
                gpu.and_then(|g| g.fan_rpm)
                    .map_or(String::from("0 RPM"), |f| format!("{} RPM", f)),
            ));

            w.set_perf_kv1_k(SharedString::from("Max clock:"));
            w.set_perf_kv1_v(SharedString::from(
                gpu.and_then(|g| g.max_clock_mhz)
                    .map_or(String::from("2.40 GHz"), |m| {
                        format!("{:.2} GHz", m / 1000.0)
                    }),
            ));
            w.set_perf_kv2_k(SharedString::from("Shared GPU mem:"));
            w.set_perf_kv2_v(SharedString::from(format_bytes(gtt_u)));
            w.set_perf_kv3_k(SharedString::from("PCI Express:"));
            w.set_perf_kv3_v(SharedString::from(
                gpu.map(|g| g.pcie_link.as_str()).unwrap_or("PCIe x16"),
            ));
            w.set_perf_kv4_k(SharedString::from("Physical location:"));
            w.set_perf_kv4_v(SharedString::from(
                gpu.map(|g| g.pci_slot.as_str()).unwrap_or("0000:00:00.0"),
            ));
            w.set_perf_kv5_k(SharedString::from("Dedicated GPU mem:"));
            w.set_perf_kv5_v(SharedString::from(format_bytes(vram_u)));
            w.set_perf_kv6_k(SharedString::from("Driver:"));
            w.set_perf_kv6_v(SharedString::from(
                gpu.map(|g| g.driver.as_str()).unwrap_or("drm"),
            ));
            w.set_perf_kv7_k(SharedString::from("Graphics API:"));
            w.set_perf_kv7_v(SharedString::from("Vulkan / OpenGL"));
            w.set_perf_kv8_k(SharedString::from("Device ID:"));
            w.set_perf_kv8_v(SharedString::from(
                gpu.map(|g| g.device_id.as_str()).unwrap_or("--"),
            ));
        }
        1 => {
            // Memory Detail View
            w.set_perf_header_title(SharedString::from("Memory"));
            w.set_perf_header_pct(SharedString::from(format!("{:.1}%", snap.memory.used_pct)));
            w.set_perf_header_sub(SharedString::from(format!(
                "{} / {}",
                format_bytes(snap.memory.used_bytes),
                format_bytes(snap.memory.total_bytes)
            )));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (snap.memory.used_pct / 100.0).clamp(0.0, 1.0),
                [184, 77, 255],
            ));
            w.set_perf_main_graph_label(SharedString::from("Memory usage"));
            let (stroke, _) = generate_svg_stroke_path(&c.mem_hist, 100.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(184, 77, 255));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 184, 77, 255));
            w.set_perf_vector_max_label(SharedString::from(format_bytes(snap.memory.total_bytes)));
            w.set_perf_vector_min_label(SharedString::from("0 GB"));
            w.set_perf_vector_current_val(SharedString::from(format!("{:.1}% ({})", snap.memory.used_pct, format_bytes(snap.memory.used_bytes))));

            w.set_perf_has_secondary_graph(true);
            w.set_perf_sec_graph_label(SharedString::from("Swap utilization"));
            w.set_perf_sec_graph_right(SharedString::from(format!(
                "{} / {}",
                format_bytes(snap.memory.swap_used_bytes),
                format_bytes(snap.memory.swap_total_bytes)
            )));
            let (sec_stroke, _) = generate_svg_stroke_path(&c.swap_hist, 100.0);
            let sec_fill = generate_svg_fill_path(&sec_stroke);
            w.set_perf_sec_vector_stroke(SharedString::from(sec_stroke));
            w.set_perf_sec_vector_fill(SharedString::from(sec_fill));
            w.set_perf_sec_vector_color(Color::from_rgb_u8(245, 150, 35));
            w.set_perf_sec_vector_fill_color(Color::from_argb_u8(40, 245, 150, 35));
            w.set_perf_sec_vector_max_label(SharedString::from(format_bytes(snap.memory.swap_total_bytes)));
            w.set_perf_sec_vector_min_label(SharedString::from("0 GB"));
            w.set_perf_sec_vector_current_val(SharedString::from(format_bytes(snap.memory.swap_used_bytes)));

            w.set_perf_m1_label(SharedString::from("In use"));
            w.set_perf_m1_val(SharedString::from(format_bytes(snap.memory.used_bytes)));
            w.set_perf_m2_label(SharedString::from("Available"));
            w.set_perf_m2_val(SharedString::from(format_bytes(
                snap.memory.available_bytes,
            )));
            w.set_perf_m3_label(SharedString::from("Committed"));
            w.set_perf_m3_val(SharedString::from(format!(
                "{} / {}",
                format_bytes(snap.memory.committed_bytes),
                format_bytes(snap.memory.commit_limit_bytes)
            )));
            w.set_perf_m4_label(SharedString::from("Cached"));
            w.set_perf_m4_val(SharedString::from(format_bytes(snap.memory.cached_bytes)));
            w.set_perf_m5_label(SharedString::from("Swap in use"));
            w.set_perf_m5_val(SharedString::from(format_bytes(
                snap.memory.swap_used_bytes,
            )));
            w.set_perf_m6_label(SharedString::from("Page faults/s"));
            w.set_perf_m6_val(SharedString::from(format!(
                "{:.0}/s",
                snap.memory.page_faults_per_sec
            )));
            w.set_perf_m7_label(SharedString::from("Zswap"));
            w.set_perf_m7_val(SharedString::from(if snap.memory.zswap_enabled {
                "Enabled"
            } else {
                "Disabled"
            }));

            w.set_perf_kv1_k(SharedString::from("Total physical:"));
            w.set_perf_kv1_v(SharedString::from(format_bytes(snap.memory.total_bytes)));
            w.set_perf_kv2_k(SharedString::from("Free memory:"));
            w.set_perf_kv2_v(SharedString::from(format_bytes(snap.memory.free_bytes)));
            w.set_perf_kv3_k(SharedString::from("Buffers:"));
            w.set_perf_kv3_v(SharedString::from(format_bytes(snap.memory.buffers_bytes)));
            w.set_perf_kv4_k(SharedString::from("Shared memory:"));
            w.set_perf_kv4_v(SharedString::from(format_bytes(snap.memory.shared_bytes)));
            w.set_perf_kv5_k(SharedString::from("Swap total:"));
            w.set_perf_kv5_v(SharedString::from(format_bytes(snap.memory.swap_total_bytes)));
            w.set_perf_kv6_k(SharedString::from("Swap free:"));
            w.set_perf_kv6_v(SharedString::from(format_bytes(snap.memory.swap_free_bytes)));
            w.set_perf_kv7_k(SharedString::from("Kernel slab:"));
            w.set_perf_kv7_v(SharedString::from(format_bytes(snap.memory.slab_bytes)));
            w.set_perf_kv8_k(SharedString::from("Major faults/s:"));
            w.set_perf_kv8_v(SharedString::from(format!("{:.0}/s", snap.memory.major_faults_per_sec)));
        }
        2 => {
            // Disk Detail View
            let disk_util = snap
                .disks
                .iter()
                .map(|d| d.busy_pct)
                .fold(0.0_f64, f64::max);
            let disk_r: f64 = snap.disks.iter().map(|d| d.read_bytes_per_sec).sum();
            let disk_w: f64 = snap.disks.iter().map(|d| d.write_bytes_per_sec).sum();
            let primary_disk = snap.disks.first();
            let disk_r_iops: f64 = snap.disks.iter().map(|d| d.read_iops).sum();
            let disk_w_iops: f64 = snap.disks.iter().map(|d| d.write_iops).sum();
            let total_read_b: u64 = snap.disks.iter().map(|d| d.total_read_bytes).sum();
            let total_written_b: u64 = snap.disks.iter().map(|d| d.total_written_bytes).sum();

            w.set_perf_header_title(SharedString::from("Disk"));
            w.set_perf_header_pct(SharedString::from(format!("{:.1}%", disk_util)));
            w.set_perf_header_sub(SharedString::from(format!(
                "{} block storage devices",
                snap.disks.len()
            )));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (disk_util / 100.0).clamp(0.02, 1.0),
                [34, 229, 139],
            ));
            w.set_perf_main_graph_label(SharedString::from("Disk Read Throughput"));
            let (stroke, max_r) = generate_svg_stroke_path(&c.disk_r_hist, 0.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(34, 229, 139));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 34, 229, 139));
            w.set_perf_vector_max_label(SharedString::from(format!("{:.1} MB/s", max_r)));
            w.set_perf_vector_min_label(SharedString::from("0 MB/s"));
            w.set_perf_vector_current_val(SharedString::from(format!("{}/s", format_bytes(disk_r as u64))));

            w.set_perf_has_secondary_graph(true);
            w.set_perf_sec_graph_label(SharedString::from("Disk Write Throughput"));
            w.set_perf_sec_graph_right(SharedString::from(format!(
                "R {}/s · W {}/s",
                format_bytes(disk_r as u64),
                format_bytes(disk_w as u64)
            )));
            let (sec_stroke, max_w) = generate_svg_stroke_path(&c.disk_w_hist, 0.0);
            let sec_fill = generate_svg_fill_path(&sec_stroke);
            w.set_perf_sec_vector_stroke(SharedString::from(sec_stroke));
            w.set_perf_sec_vector_fill(SharedString::from(sec_fill));
            w.set_perf_sec_vector_color(Color::from_rgb_u8(54, 197, 255));
            w.set_perf_sec_vector_fill_color(Color::from_argb_u8(40, 54, 197, 255));
            w.set_perf_sec_vector_max_label(SharedString::from(format!("{:.1} MB/s", max_w)));
            w.set_perf_sec_vector_min_label(SharedString::from("0 MB/s"));
            w.set_perf_sec_vector_current_val(SharedString::from(format!("{}/s", format_bytes(disk_w as u64))));

            w.set_perf_m1_label(SharedString::from("Active time"));
            w.set_perf_m1_val(SharedString::from(format!("{:.1}%", disk_util)));
            w.set_perf_m2_label(SharedString::from("Read rate"));
            w.set_perf_m2_val(SharedString::from(format!("{}/s", format_bytes(disk_r as u64))));
            w.set_perf_m3_label(SharedString::from("Write rate"));
            w.set_perf_m3_val(SharedString::from(format!("{}/s", format_bytes(disk_w as u64))));
            w.set_perf_m4_label(SharedString::from("Total IOPS"));
            w.set_perf_m4_val(SharedString::from(format!("{:.0} IOPS", disk_r_iops + disk_w_iops)));
            w.set_perf_m5_label(SharedString::from("Total read"));
            w.set_perf_m5_val(SharedString::from(format_bytes(total_read_b)));
            w.set_perf_m6_label(SharedString::from("Total written"));
            w.set_perf_m6_val(SharedString::from(format_bytes(total_written_b)));
            w.set_perf_m7_label(SharedString::from("Total disks"));
            w.set_perf_m7_val(SharedString::from(snap.disks.len().to_string()));

            w.set_perf_kv1_k(SharedString::from("Primary disk:"));
            w.set_perf_kv1_v(SharedString::from(primary_disk.map(|d| d.name.as_str()).unwrap_or("--")));
            w.set_perf_kv2_k(SharedString::from("Drive model:"));
            w.set_perf_kv2_v(SharedString::from(primary_disk.map(|d| d.model.as_str()).unwrap_or("Block Storage Device")));
            w.set_perf_kv3_k(SharedString::from("Capacity:"));
            w.set_perf_kv3_v(SharedString::from(primary_disk.map(|d| format_bytes(d.capacity_bytes)).unwrap_or_else(|| "--".into())));
            w.set_perf_kv4_k(SharedString::from("Drive kind:"));
            w.set_perf_kv4_v(SharedString::from(primary_disk.map(|d| d.kind.as_str()).unwrap_or("NVMe / SSD")));
            w.set_perf_kv5_k(SharedString::from("Read IOPS:"));
            w.set_perf_kv5_v(SharedString::from(format!("{:.0} ops/s", disk_r_iops)));
            w.set_perf_kv6_k(SharedString::from("Write IOPS:"));
            w.set_perf_kv6_v(SharedString::from(format!("{:.0} ops/s", disk_w_iops)));
            w.set_perf_kv7_k(SharedString::from("Mount points:"));
            w.set_perf_kv7_v(SharedString::from(primary_disk.map(|d| d.mount_points.join(", ")).unwrap_or_else(|| "/".into())));
            w.set_perf_kv8_k(SharedString::from("Disks online:"));
            w.set_perf_kv8_v(SharedString::from(snap.disks.len().to_string()));
        }
        3 => {
            // Network Detail View
            let net = snap
                .networks
                .iter()
                .find(|n| n.name != "lo")
                .or_else(|| snap.networks.first());
            let name = net.map(|n| n.name.as_str()).unwrap_or("Network");
            let rx = net.map(|n| n.rx_bytes_per_sec).unwrap_or(0.0);
            let tx = net.map(|n| n.tx_bytes_per_sec).unwrap_or(0.0);
            w.set_perf_header_title(SharedString::from(name));
            w.set_perf_header_pct(SharedString::from(format!(
                "{}/s",
                format_bytes((rx + tx) as u64)
            )));
            w.set_perf_header_sub(SharedString::from(
                net.map(|n| format!("IPv4 {} · MAC {}", n.ipv4, n.mac_address))
                    .unwrap_or_default(),
            ));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                ((rx + tx) / (33.3 * 1024.0 * 1024.0)).clamp(0.03, 1.0),
                [25, 181, 254],
            ));
            w.set_perf_main_graph_label(SharedString::from("Receive Throughput"));
            let (stroke, max_rx) = generate_svg_stroke_path(&c.net_rx_hist, 0.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(25, 181, 254));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 25, 181, 254));
            w.set_perf_vector_max_label(SharedString::from(format!("{:.1} KB/s", max_rx)));
            w.set_perf_vector_min_label(SharedString::from("0 KB/s"));
            w.set_perf_vector_current_val(SharedString::from(format!("{}/s", format_bytes(rx as u64))));

            w.set_perf_has_secondary_graph(true);
            w.set_perf_sec_graph_label(SharedString::from("Send Throughput"));
            w.set_perf_sec_graph_right(SharedString::from(format!(
                "RX {}/s · TX {}/s",
                format_bytes(rx as u64),
                format_bytes(tx as u64)
            )));
            let (sec_stroke, max_tx) = generate_svg_stroke_path(&c.net_tx_hist, 0.0);
            let sec_fill = generate_svg_fill_path(&sec_stroke);
            w.set_perf_sec_vector_stroke(SharedString::from(sec_stroke));
            w.set_perf_sec_vector_fill(SharedString::from(sec_fill));
            w.set_perf_sec_vector_color(Color::from_rgb_u8(184, 77, 255));
            w.set_perf_sec_vector_fill_color(Color::from_argb_u8(40, 184, 77, 255));
            w.set_perf_sec_vector_max_label(SharedString::from(format!("{:.1} KB/s", max_tx)));
            w.set_perf_sec_vector_min_label(SharedString::from("0 KB/s"));
            w.set_perf_sec_vector_current_val(SharedString::from(format!("{}/s", format_bytes(tx as u64))));

            w.set_perf_m1_label(SharedString::from("Receive rate"));
            w.set_perf_m1_val(SharedString::from(format!("{}/s", format_bytes(rx as u64))));
            w.set_perf_m2_label(SharedString::from("Send rate"));
            w.set_perf_m2_val(SharedString::from(format!("{}/s", format_bytes(tx as u64))));
            w.set_perf_m3_label(SharedString::from("Total throughput"));
            w.set_perf_m3_val(SharedString::from(format!("{}/s", format_bytes((rx + tx) as u64))));
            w.set_perf_m4_label(SharedString::from("Packets/s"));
            w.set_perf_m4_val(SharedString::from(format!("{:.0}", net.map(|n| n.rx_packets_per_sec + n.tx_packets_per_sec).unwrap_or(0.0))));
            w.set_perf_m5_label(SharedString::from("Total received"));
            w.set_perf_m5_val(SharedString::from(format_bytes(net.map(|n| n.total_rx_bytes).unwrap_or(0))));
            w.set_perf_m6_label(SharedString::from("Total sent"));
            w.set_perf_m6_val(SharedString::from(format_bytes(net.map(|n| n.total_tx_bytes).unwrap_or(0))));
            w.set_perf_m7_label(SharedString::from("Link state"));
            w.set_perf_m7_val(SharedString::from(net.map(|n| n.oper_state.as_str()).unwrap_or("up")));

            w.set_perf_kv1_k(SharedString::from("Adapter name:"));
            w.set_perf_kv1_v(SharedString::from(name));
            w.set_perf_kv2_k(SharedString::from("Adapter type:"));
            w.set_perf_kv2_v(SharedString::from(net.map(|n| n.kind.as_str()).unwrap_or("Ethernet")));
            w.set_perf_kv3_k(SharedString::from("IPv4 address:"));
            w.set_perf_kv3_v(SharedString::from(net.map(|n| n.ipv4.as_str()).unwrap_or("--")));
            w.set_perf_kv4_k(SharedString::from("IPv6 address:"));
            w.set_perf_kv4_v(SharedString::from(net.map(|n| n.ipv6.as_str()).unwrap_or("--")));
            w.set_perf_kv5_k(SharedString::from("MAC address:"));
            w.set_perf_kv5_v(SharedString::from(net.map(|n| n.mac_address.as_str()).unwrap_or("--")));
            w.set_perf_kv6_k(SharedString::from("Link speed:"));
            w.set_perf_kv6_v(SharedString::from(net.and_then(|n| n.speed_mbps).map(|s| format!("{} Mbps", s)).unwrap_or_else(|| "1000 Mbps".into())));
            w.set_perf_kv7_k(SharedString::from("Active sockets:"));
            w.set_perf_kv7_v(SharedString::from(snap.connections.len().to_string()));
            w.set_perf_kv8_k(SharedString::from("Network status:"));
            w.set_perf_kv8_v(SharedString::from("Online / Operational"));
        }
        5 => {
            // Energy Detail View
            let pwr = snap
                .cpu
                .package_power_watts
                .or_else(|| snap.gpus.first().and_then(|g| g.power_watts))
                .unwrap_or(45.0);
            w.set_perf_header_title(SharedString::from("Energy"));
            w.set_perf_header_pct(SharedString::from(format!("{:.1} W", pwr)));
            w.set_perf_header_sub(SharedString::from("Whole machine power telemetry"));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (pwr / 200.0).clamp(0.05, 1.0),
                [245, 200, 35],
            ));
            w.set_perf_has_secondary_graph(false);
            w.set_perf_main_graph_label(SharedString::from("Power Draw History"));
            let (stroke, max_pwr) = generate_svg_stroke_path(&c.power_hist, 0.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(245, 200, 35));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 245, 200, 35));
            w.set_perf_vector_max_label(SharedString::from(format!("{:.0} W", max_pwr.max(100.0))));
            w.set_perf_vector_min_label(SharedString::from("0 W"));
            w.set_perf_vector_current_val(SharedString::from(format!("{:.1} W", pwr)));

            let cpu_pwr = snap.cpu.package_power_watts.unwrap_or(pwr * 0.7);
            let gpu_pwr = snap.gpus.first().and_then(|g| g.power_watts).unwrap_or(pwr * 0.3);

            w.set_perf_m1_label(SharedString::from("Package power"));
            w.set_perf_m1_val(SharedString::from(format!("{:.1} W", pwr)));
            w.set_perf_m2_label(SharedString::from("CPU power"));
            w.set_perf_m2_val(SharedString::from(format!("{:.1} W", cpu_pwr)));
            w.set_perf_m3_label(SharedString::from("GPU power"));
            w.set_perf_m3_val(SharedString::from(format!("{:.1} W", gpu_pwr)));
            w.set_perf_m4_label(SharedString::from("Avg frequency"));
            w.set_perf_m4_val(SharedString::from(format!("{:.2} GHz", snap.cpu.avg_freq_mhz / 1000.0)));
            w.set_perf_m5_label(SharedString::from("Energy driver"));
            w.set_perf_m5_val(SharedString::from("RAPL / powercap"));
            w.set_perf_m6_label(SharedString::from("Governor"));
            w.set_perf_m6_val(SharedString::from(c.cpu_governor.as_str()));
            w.set_perf_m7_label(SharedString::from("Power profile"));
            w.set_perf_m7_val(SharedString::from("Balanced Performance"));

            w.set_perf_kv1_k(SharedString::from("Telemetry source:"));
            w.set_perf_kv1_v(SharedString::from("Intel RAPL / AMD sysfs"));
            w.set_perf_kv2_k(SharedString::from("Base clock:"));
            w.set_perf_kv2_v(SharedString::from(format!("{:.2} GHz", c.cpu_base_ghz)));
            w.set_perf_kv3_k(SharedString::from("Max turbo clock:"));
            w.set_perf_kv3_v(SharedString::from(format!("{:.2} GHz", snap.cpu.max_freq_mhz / 1000.0)));
            w.set_perf_kv4_k(SharedString::from("Frequency driver:"));
            w.set_perf_kv4_v(SharedString::from(c.cpu_driver.as_str()));
            w.set_perf_kv5_k(SharedString::from("Sockets:"));
            w.set_perf_kv5_v(SharedString::from("1"));
            w.set_perf_kv6_k(SharedString::from("Thermal design power:"));
            w.set_perf_kv6_v(SharedString::from("TDP 65W - 125W"));
            w.set_perf_kv7_k(SharedString::from("AC power status:"));
            w.set_perf_kv7_v(SharedString::from("Plugged In / AC"));
            w.set_perf_kv8_k(SharedString::from("Energy efficiency:"));
            w.set_perf_kv8_v(SharedString::from("Active Energy Tuning"));
        }
        _ => {
            // Thermals Detail View
            let temp = snap.cpu.package_temp_c.unwrap_or(55.0);
            w.set_perf_header_title(SharedString::from("Thermals"));
            w.set_perf_header_pct(SharedString::from(format!("{:.0} °C", temp)));
            w.set_perf_header_sub(SharedString::from("Package & board thermal sensors"));
            w.set_perf_header_led_bar(render_horizontal_led_bar(
                680,
                32,
                52,
                (temp / 100.0).clamp(0.05, 1.0),
                [245, 150, 35],
            ));
            w.set_perf_has_secondary_graph(false);
            w.set_perf_main_graph_label(SharedString::from("Temperature History (°C)"));
            let (stroke, _) = generate_svg_stroke_path(&c.cpu_temp_hist, 100.0);
            let fill = generate_svg_fill_path(&stroke);
            w.set_perf_vector_stroke(SharedString::from(stroke));
            w.set_perf_vector_fill(SharedString::from(fill));
            w.set_perf_vector_has_kernel(false);
            w.set_perf_vector_color(Color::from_rgb_u8(245, 150, 35));
            w.set_perf_vector_fill_color(Color::from_argb_u8(40, 245, 150, 35));
            w.set_perf_vector_max_label(SharedString::from("100 °C"));
            w.set_perf_vector_min_label(SharedString::from("0 °C"));
            w.set_perf_vector_current_val(SharedString::from(format!("{:.0} °C", temp)));

            let max_temp = snap.sensors.iter()
                .flat_map(|g| g.readings.iter())
                .filter(|r| r.category == "Temperature")
                .map(|r| r.current)
                .fold(temp, f64::max);
            let fan_rpm = snap.sensors.iter()
                .flat_map(|g| g.readings.iter())
                .find(|r| r.category == "Fan")
                .map(|r| r.current)
                .or_else(|| snap.gpus.first().and_then(|g| g.fan_rpm.map(|f| f as f64)))
                .unwrap_or(0.0);
            let fan_str = if fan_rpm > 0.0 { format!("{:.0} RPM", fan_rpm) } else { "Passive / Silent".to_string() };

            w.set_perf_m1_label(SharedString::from("Package temp"));
            w.set_perf_m1_val(SharedString::from(format!("{:.0} °C", temp)));
            w.set_perf_m2_label(SharedString::from("Max hot-spot"));
            w.set_perf_m2_val(SharedString::from(format!("{:.0} °C", max_temp)));
            w.set_perf_m3_label(SharedString::from("Fan speed"));
            w.set_perf_m3_val(SharedString::from(fan_str));
            w.set_perf_m4_label(SharedString::from("Thermal state"));
            w.set_perf_m4_val(SharedString::from(if temp > 85.0 { "HOT / Warning" } else if temp > 70.0 { "Warm" } else { "Normal / Cool" }));
            w.set_perf_m5_label(SharedString::from("Active sensors"));
            w.set_perf_m5_val(SharedString::from(snap.sensors.iter().map(|g| g.readings.len()).sum::<usize>().to_string()));
            w.set_perf_m6_label(SharedString::from("Thermal throttling"));
            w.set_perf_m6_val(SharedString::from("None (Operating within limits)"));
            w.set_perf_m7_label(SharedString::from("Cooling policy"));
            w.set_perf_m7_val(SharedString::from("Dynamic fan curve"));

            w.set_perf_kv1_k(SharedString::from("Primary sensor:"));
            w.set_perf_kv1_v(SharedString::from(snap.sensors.first().map(|g| g.chip_name.as_str()).unwrap_or("coretemp / k10temp")));
            w.set_perf_kv2_k(SharedString::from("Tjunction limit:"));
            w.set_perf_kv2_v(SharedString::from("100 °C"));
            w.set_perf_kv3_k(SharedString::from("Trip point 0:"));
            w.set_perf_kv3_v(SharedString::from("85 °C (Fan ramp)"));
            w.set_perf_kv4_k(SharedString::from("Trip point 1:"));
            w.set_perf_kv4_v(SharedString::from("105 °C (Emergency shutdown)"));
            w.set_perf_kv5_k(SharedString::from("Board thermal zones:"));
            w.set_perf_kv5_v(SharedString::from(snap.sensors.len().to_string()));
            w.set_perf_kv6_k(SharedString::from("Fan controller:"));
            w.set_perf_kv6_v(SharedString::from("hwmon PWM controller"));
            w.set_perf_kv7_k(SharedString::from("Thermal governor:"));
            w.set_perf_kv7_v(SharedString::from("step_wise"));
            w.set_perf_kv8_k(SharedString::from("Cooling status:"));
            w.set_perf_kv8_v(SharedString::from("Optimal"));
        }
    }
}

fn sync_filtered_views(w: &MainWindow, c: &GuiController) {
    let snap = &c.last_snap;

    // 1. Processes
    let p_query = w.get_proc_filter().to_lowercase();
    let hide_k = w.get_proc_hide_kthreads();
    let tree_mode = w.get_proc_tree_view();

    let mut filtered_procs: Vec<&crate::model::ProcessEntry> = snap
        .processes
        .iter()
        .filter(|p| {
            if hide_k && p.is_kernel_thread {
                return false;
            }
            if p_query.is_empty() {
                return true;
            }
            p.name.to_lowercase().contains(&p_query)
                || p.user.to_lowercase().contains(&p_query)
                || p.cmdline.to_lowercase().contains(&p_query)
                || p.pid.to_string().contains(&p_query)
        })
        .collect();

    let mut p_rows = Vec::with_capacity(filtered_procs.len().min(300));
    if tree_mode && p_query.is_empty() {
        let mut by_ppid: std::collections::BTreeMap<u32, Vec<&crate::model::ProcessEntry>> =
            std::collections::BTreeMap::new();
        let pid_set: std::collections::HashSet<u32> = filtered_procs.iter().map(|p| p.pid).collect();
        let mut roots = Vec::new();
        for p in &filtered_procs {
            if p.ppid == 0 || !pid_set.contains(&p.ppid) || p.ppid == p.pid {
                roots.push(*p);
            } else {
                by_ppid.entry(p.ppid).or_default().push(*p);
            }
        }
        fn walk_tree<'a>(
            node: &'a crate::model::ProcessEntry,
            by_ppid: &std::collections::BTreeMap<u32, Vec<&'a crate::model::ProcessEntry>>,
            depth: usize,
            out: &mut Vec<ProcRow>,
        ) {
            if out.len() >= 300 {
                return;
            }
            let prefix = if depth == 0 {
                String::new()
            } else {
                format!("{}└─ ", "  ".repeat(depth.min(6) - 1))
            };
            let disp = format!("{}{}", prefix, node.name);
            out.push(to_proc_row(node, Some(&disp)));
            if let Some(children) = by_ppid.get(&node.pid) {
                for ch in children {
                    walk_tree(ch, by_ppid, depth + 1, out);
                }
            }
        }
        for r in roots {
            walk_tree(r, &by_ppid, 0, &mut p_rows);
        }
    } else {
        filtered_procs.truncate(300);
        for p in filtered_procs {
            p_rows.push(to_proc_row(p, None));
        }
    }
    w.set_processes(ModelRc::new(VecModel::from(p_rows)));

    // 2. Connections
    let c_query = w.get_conn_filter().to_lowercase();
    let pub_only = w.get_conn_public_only();
    let c_rows: Vec<ConnRow> = snap
        .connections
        .iter()
        .filter(|cn| {
            if pub_only && !cn.is_public_listener {
                return false;
            }
            if c_query.is_empty() {
                return true;
            }
            cn.protocol.to_lowercase().contains(&c_query)
                || cn.local_addr.to_lowercase().contains(&c_query)
                || cn.remote_addr.to_lowercase().contains(&c_query)
                || cn.process_name.to_lowercase().contains(&c_query)
                || cn.local_port.to_string().contains(&c_query)
        })
        .take(300)
        .map(|cn| ConnRow {
            proto: SharedString::from(cn.protocol.as_str()),
            local_ep: SharedString::from(if cn.local_port > 0 {
                format!("{}:{}", cn.local_addr, cn.local_port)
            } else {
                cn.local_addr.clone()
            }),
            remote_ep: SharedString::from(if cn.remote_port > 0 {
                format!("{}:{}", cn.remote_addr, cn.remote_port)
            } else {
                cn.remote_addr.clone()
            }),
            state: SharedString::from(cn.state.as_str()),
            pid_str: SharedString::from(cn.pid.map_or(String::from("-"), |p| p.to_string())),
            proc_name: SharedString::from(cn.process_name.as_str()),
            public_badge: SharedString::from(if cn.is_public_listener {
                "PUBLIC LISTENER"
            } else {
                "Local / Bound"
            }),
            is_public: cn.is_public_listener,
        })
        .collect();
    w.set_connections(ModelRc::new(VecModel::from(c_rows)));

    // 3. Drivers
    let d_query = w.get_drv_filter().to_lowercase();
    let d_rows: Vec<DriverRow> = c
        .sampler()
        .drivers
        .iter()
        .filter(|m| {
            if d_query.is_empty() {
                return true;
            }
            m.name.to_lowercase().contains(&d_query)
                || m.module_path.to_lowercase().contains(&d_query)
                || m.used_by.iter().any(|u| u.to_lowercase().contains(&d_query))
        })
        .take(300)
        .map(|m| DriverRow {
            name: SharedString::from(m.name.as_str()),
            size_str: SharedString::from(format_bytes(m.size_bytes)),
            refs_str: SharedString::from(m.ref_count.to_string()),
            state: SharedString::from(m.state.as_str()),
            in_tree: SharedString::from(if m.in_tree { "In-Tree" } else { "External" }),
            used_by: SharedString::from(if m.used_by.is_empty() {
                String::from("-")
            } else {
                m.used_by.join(", ")
            }),
            path: SharedString::from(m.module_path.as_str()),
        })
        .collect();
    w.set_drv_summary(SharedString::from(format!(
        "Showing {} / {} loaded drivers",
        d_rows.len(),
        c.sampler().drivers.len()
    )));
    w.set_drivers(ModelRc::new(VecModel::from(d_rows)));

    // 4. Catalog Tabs (4=Startup apps, 5=Users, 6=Services, 10=Installed Apps)
    let cat_query = w.get_catalog_filter().to_lowercase();
    let tab = w.get_active_tab();
    let mut cat_rows = Vec::new();
    match tab {
        4 => {
            for st in &c.sampler().startup_apps {
                if !cat_query.is_empty()
                    && !st.name.to_lowercase().contains(&cat_query)
                    && !st.exec.to_lowercase().contains(&cat_query)
                {
                    continue;
                }
                cat_rows.push(CatalogRow {
                    col1: SharedString::from(st.name.as_str()),
                    col2: SharedString::from(if st.enabled { "ENABLED" } else { "DISABLED" }),
                    col3: SharedString::from(st.scope.as_str()),
                    col4: SharedString::from(st.comment.as_str()),
                    col5: SharedString::from(st.exec.as_str()),
                    col6: SharedString::from(st.path.as_str()),
                });
            }
        }
        5 => {
            for u in &c.sampler().users {
                cat_rows.push(CatalogRow {
                    col1: SharedString::from(u.username.as_str()),
                    col2: SharedString::from(format!("UID {}", u.uid)),
                    col3: SharedString::from(u.tty.as_str()),
                    col4: SharedString::from(u.remote_host.as_str()),
                    col5: SharedString::from(format!(
                        "{} procs ({})",
                        u.process_count,
                        format_bytes(u.rss_bytes)
                    )),
                    col6: SharedString::from(format!(
                        "CPU {:.1}% · Login {}",
                        u.cpu_pct, u.login_time
                    )),
                });
            }
        }
        6 => {
            for s in &c.sampler().services {
                if !cat_query.is_empty()
                    && !s.unit.to_lowercase().contains(&cat_query)
                    && !s.description.to_lowercase().contains(&cat_query)
                {
                    continue;
                }
                if cat_rows.len() >= 300 {
                    break;
                }
                cat_rows.push(CatalogRow {
                    col1: SharedString::from(s.unit.as_str()),
                    col2: SharedString::from(s.active_state.as_str()),
                    col3: SharedString::from(s.sub_state.as_str()),
                    col4: SharedString::from(s.load_state.as_str()),
                    col5: SharedString::from("systemd"),
                    col6: SharedString::from(s.description.as_str()),
                });
            }
        }
        _ => {
            for app in &c.sampler().installed_apps {
                if !cat_query.is_empty()
                    && !app.name.to_lowercase().contains(&cat_query)
                    && !app.description.to_lowercase().contains(&cat_query)
                {
                    continue;
                }
                if cat_rows.len() >= 300 {
                    break;
                }
                cat_rows.push(CatalogRow {
                    col1: SharedString::from(app.name.as_str()),
                    col2: SharedString::from(app.version.as_str()),
                    col3: SharedString::from(app.source.as_str()),
                    col4: SharedString::from(app.section.as_str()),
                    col5: SharedString::from(format_bytes(app.installed_size_kb * 1024)),
                    col6: SharedString::from(app.description.as_str()),
                });
            }
        }
    }
    w.set_catalog_rows(ModelRc::new(VecModel::from(cat_rows)));
}

fn sync_disk_space_models(w: &MainWindow, c: &GuiController) {
    let smp = c.sampler();
    let ds = &smp.disk_space;
    w.set_disk_scan_summary(SharedString::from(format!(
        "Root: {} | Used: {} / {} | Scanned {} ({} files in {} ms)",
        ds.root_path,
        format_bytes(ds.fs_used_bytes),
        format_bytes(ds.fs_total_bytes),
        format_bytes(ds.scanned_bytes),
        ds.scanned_files,
        ds.scan_duration_ms
    )));

    let dirs: Vec<DiskUsageRow> = ds
        .top_directories
        .iter()
        .enumerate()
        .map(|(i, d)| DiskUsageRow {
            rank: SharedString::from(format!("#{}", i + 1)),
            name: SharedString::from(d.name.as_str()),
            path: SharedString::from(d.path.as_str()),
            size_str: SharedString::from(format_bytes(d.size_bytes)),
            files_str: SharedString::from(format!("{} files", d.file_count)),
            pct_str: SharedString::from(format!("{:.1}%", d.pct_of_root)),
            ratio: (d.pct_of_root / 100.0).clamp(0.0, 1.0) as f32,
        })
        .collect();
    w.set_disk_dirs(ModelRc::new(VecModel::from(dirs)));

    let cats: Vec<DiskUsageRow> = ds
        .categories
        .iter()
        .enumerate()
        .map(|(i, cat)| DiskUsageRow {
            rank: SharedString::from(format!("#{}", i + 1)),
            name: SharedString::from(cat.category.as_str()),
            path: SharedString::from(""),
            size_str: SharedString::from(format_bytes(cat.size_bytes)),
            files_str: SharedString::from(format!("{} files", cat.file_count)),
            pct_str: SharedString::from(format!("{:.1}%", cat.pct_of_root)),
            ratio: (cat.pct_of_root / 100.0).clamp(0.0, 1.0) as f32,
        })
        .collect();
    w.set_disk_categories(ModelRc::new(VecModel::from(cats)));
}

fn sync_flight_view(w: &MainWindow, c: &GuiController) {
    let smp = c.sampler();
    let trace = &smp.flight_recorder.trace;
    let samples = &trace.samples;
    if samples.is_empty() {
        return;
    }
    let idx = (w.get_flight_scrub_index() as usize).min(samples.len() - 1);
    let s = &samples[idx];
    w.set_flight_frame_summary(SharedString::from(format!(
        "Frame #{} / {}  |  Timestamp: {} ms  |  Schema: {} v{}\n\
         CPU Utilization: {:.1}%  |  Average Clock: {:.0} MHz  |  Package Temp: {:.1} °C\n\
         Memory Used: {} ({:.1}%)  |  Swap Used: {}  |  System Pressure: {} ({:.1}%)\n\
         Storage I/O: Read {}/s, Write {}/s  |  Network I/O: RX {}/s, TX {}/s\n\
         Top CPU Process: {} (PID {}) at {:.1}% CPU",
        s.index,
        samples.len(),
        s.timestamp_epoch_ms,
        trace.schema,
        trace.version,
        s.cpu_total_pct,
        s.cpu_avg_freq_mhz,
        s.cpu_temp_c,
        format_bytes(s.mem_used_bytes),
        s.mem_used_pct,
        format_bytes(s.swap_used_bytes),
        s.pressure_level,
        s.pressure_score,
        format_bytes(s.disk_read_bps as u64),
        format_bytes(s.disk_write_bps as u64),
        format_bytes(s.net_rx_bps as u64),
        format_bytes(s.net_tx_bps as u64),
        s.top_process_name,
        s.top_process_pid,
        s.top_process_cpu_pct
    )));

    let cpu_q: VecDeque<f64> = samples.iter().map(|x| x.cpu_total_pct).collect();
    let mem_q: VecDeque<f64> = samples.iter().map(|x| x.mem_used_pct).collect();
    let pr_q: VecDeque<f64> = samples.iter().map(|x| x.pressure_score).collect();
    w.set_flight_graph(render_3series_cpu_overview_graph(
        820,
        160,
        &cpu_q,
        &pr_q,
        &mem_q,
        c.settings.show_graph_grid,
    ));
}

fn to_proc_row(p: &crate::model::ProcessEntry, custom_disp: Option<&str>) -> ProcRow {
    ProcRow {
        pid: p.pid as i32,
        ppid: p.ppid as i32,
        name: SharedString::from(p.name.as_str()),
        display_name: SharedString::from(custom_disp.unwrap_or(p.name.as_str())),
        user: SharedString::from(p.user.as_str()),
        state: SharedString::from(p.state.to_string()),
        cpu_str: SharedString::from(format!("{:.1}%", p.cpu_pct)),
        gpu_str: SharedString::from("—"),
        mem_pct_str: SharedString::from(format!("{:.1}%", p.mem_pct)),
        rss_str: SharedString::from(format_bytes(p.mem_rss_bytes)),
        io_read_str: SharedString::from(format!("{}/s", format_bytes(p.read_bytes_per_sec as u64))),
        io_write_str: SharedString::from(format!(
            "{}/s",
            format_bytes(p.write_bytes_per_sec as u64)
        )),
        threads: p.threads as i32,
        nice: p.nice,
        cmdline: SharedString::from(p.cmdline.as_str()),
        highlight_kind: 0,
    }
}

fn format_uptime_dhms(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{}:{:02}:{:02}:{:02}", d, h, m, s)
}

fn apply_settings_to_window(w: &MainWindow, s: &AppSettings) {
    let t = w.global::<Theme>();
    let to_col = |rgb: [u8; 3]| Color::from_rgb_u8(rgb[0], rgb[1], rgb[2]);
    let blend = |c1: [u8; 3], c2: [u8; 3], f: f32| -> Color {
        let f_clamped = f.clamp(0.0, 1.0);
        let r = (c1[0] as f32 * (1.0 - f_clamped) + c2[0] as f32 * f_clamped).clamp(0.0, 255.0) as u8;
        let g = (c1[1] as f32 * (1.0 - f_clamped) + c2[1] as f32 * f_clamped).clamp(0.0, 255.0) as u8;
        let b = (c1[2] as f32 * (1.0 - f_clamped) + c2[2] as f32 * f_clamped).clamp(0.0, 255.0) as u8;
        Color::from_rgb_u8(r, g, b)
    };

    t.set_bg_main(to_col(s.theme.bg_main));
    t.set_bg_panel(to_col(s.theme.bg_panel));
    t.set_bg_card(to_col(s.theme.bg_card));
    t.set_bg_header(to_col(s.theme.bg_panel));
    t.set_border(to_col(s.theme.border));
    t.set_text_main(to_col(s.theme.text_main));
    t.set_text_dim(to_col(s.theme.text_dim));
    t.set_accent_primary(to_col(s.theme.accent_primary));
    t.set_accent_secondary(to_col(s.theme.accent_secondary));
    t.set_accent_warn(to_col(s.theme.accent_warn));
    t.set_accent_danger(to_col(s.theme.accent_danger));
    t.set_font_family(SharedString::from(s.font_family.as_str()));

    // Dynamic UI & Menu colors matched harmoniously to active theme
    t.set_row_alt(blend(s.theme.bg_main, s.theme.bg_card, 0.45));
    t.set_nav_selected(blend(s.theme.bg_panel, s.theme.accent_primary, 0.22));
    t.set_nav_border(blend(s.theme.border, s.theme.accent_primary, 0.40));
    t.set_menu_bg(blend(s.theme.bg_panel, s.theme.bg_card, 0.70));
    t.set_menu_border(blend(s.theme.border, s.theme.accent_primary, 0.35));
    t.set_menu_hover(blend(s.theme.bg_card, s.theme.accent_primary, 0.20));

    if s.theme.preset_name.contains("Fluent") {
        t.set_ui_style(1);
        t.set_card_radius(8.0);
        t.set_button_radius(6.0);
    } else if s.theme.preset_name.contains("KDE") || s.theme.preset_name.contains("Breeze") {
        t.set_ui_style(2);
        t.set_card_radius(4.0);
        t.set_button_radius(3.0);
    } else {
        t.set_ui_style(0);
        t.set_card_radius(6.0);
        t.set_button_radius(4.0);
    }

    let base = s.base_font_size.clamp(9, 22) as f32;
    t.set_font_size_sm((base - 1.0).max(9.0));
    t.set_font_size_base(base);
    t.set_font_size_md(base + 2.0);
    t.set_font_size_lg(base + 5.0);
    t.set_font_size_xl(base + 9.0);
    t.set_row_height(if s.compact_rows { 20.0 } else { 25.0 });
    t.set_show_grid(s.show_graph_grid);

    w.set_current_theme_name(SharedString::from(s.theme.preset_name.as_str()));
    w.set_custom_primary_hex(SharedString::from(rgb_to_hex(s.theme.accent_primary)));
    w.set_custom_secondary_hex(SharedString::from(rgb_to_hex(s.theme.accent_secondary)));
    w.set_custom_bg_hex(SharedString::from(rgb_to_hex(s.theme.bg_main)));
    w.set_custom_card_hex(SharedString::from(rgb_to_hex(s.theme.bg_card)));
    w.set_custom_text_hex(SharedString::from(rgb_to_hex(s.theme.text_main)));
    w.set_custom_font_family(SharedString::from(s.font_family.as_str()));
    w.set_custom_font_size(s.base_font_size as i32);
    w.set_custom_refresh_ms(s.refresh_ms as i32);
    w.set_custom_compact_rows(s.compact_rows);
    w.set_custom_show_grid(s.show_graph_grid);
    w.set_chart_fps(s.chart_fps as i32);
    if s.chart_fps == 0 {
        w.global::<ChartClock>().set_phase(1.0);
    }
}

// ============================================================================
// CUSTOM PIXEL-BUFFER RENDERERS (Segmented LED Bars, Multi-Graphs & ECG Logo)
// ============================================================================

/// Renders the 4 vertical glowing segmented LED bar columns on the Summary `SYS` card:
/// Column 0: CPU (Green), Column 1: Clock (Red), Column 2: Temp (Yellow), Column 3: GPU (Cyan)
fn render_4_vertical_led_bars(
    cpu_r: f64,
    clock_r: f64,
    temp_r: f64,
    gpu_r: f64,
) -> Image {
    let w: u32 = 180;
    let h: u32 = 210;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();

    for px in slice.iter_mut() {
        *px = Rgba8Pixel { r: 11, g: 14, b: 18, a: 255 };
    }

    let cols = [
        (cpu_r, [34u8, 255, 110]),
        (clock_r, [255u8, 60, 50]),
        (temp_r, [255u8, 205, 35]),
        (gpu_r, [30u8, 190, 255]),
    ];
    let num_segs = 30usize;
    let col_w = w as usize / 4;
    let seg_w = 22usize;
    let top_pad = 6usize;
    let bot_pad = 6usize;
    let usable_h = (h as usize).saturating_sub(top_pad + bot_pad);
    let step_y = usable_h / num_segs;
    let seg_h = step_y.saturating_sub(2).max(3);

    for (c_idx, (ratio, col)) in cols.iter().enumerate() {
        let lit_count = ((ratio.clamp(0.0, 1.0) * num_segs as f64).round() as usize).max(1);
        let cx_start = c_idx * col_w + (col_w.saturating_sub(seg_w)) / 2;

        for s in 0..num_segs {
            // s=0 is bottom segment, s=num_segs-1 is top segment
            let y_top = h as usize - bot_pad - (s + 1) * step_y;
            let is_lit = s < lit_count;

            if is_lit {
                // Outer soft glow halo
                for gy in y_top.saturating_sub(2)..(y_top + seg_h + 2).min(h as usize) {
                    for gx in cx_start.saturating_sub(3)..(cx_start + seg_w + 3).min(w as usize) {
                        let p = &mut slice[gy * w as usize + gx];
                        p.r = p.r.saturating_add(col[0] / 7);
                        p.g = p.g.saturating_add(col[1] / 7);
                        p.b = p.b.saturating_add(col[2] / 7);
                    }
                }
            }

            for y in y_top..(y_top + seg_h).min(h as usize) {
                for x in cx_start..(cx_start + seg_w).min(w as usize) {
                    let is_border = y == y_top
                        || y + 1 == y_top + seg_h
                        || x == cx_start
                        || x + 1 == cx_start + seg_w;
                    let px = &mut slice[y * w as usize + x];
                    if is_lit {
                        if is_border {
                            *px = Rgba8Pixel {
                                r: col[0],
                                g: col[1],
                                b: col[2],
                                a: 255,
                            };
                        } else {
                            *px = Rgba8Pixel {
                                r: col[0].saturating_add(35),
                                g: col[1].saturating_add(35),
                                b: col[2].saturating_add(35),
                                a: 255,
                            };
                        }
                    } else {
                        *px = Rgba8Pixel {
                            r: (col[0] as u16 / 9) as u8 + 8,
                            g: (col[1] as u16 / 9) as u8 + 10,
                            b: (col[2] as u16 / 9) as u8 + 13,
                            a: 255,
                        };
                    }
                }
            }
        }
    }

    Image::from_rgba8(buf)
}

/// Renders the single vertical purple segmented LED bar on the Summary `Memory Utilization` card.
fn render_single_vertical_led_bar(ratio: f64, col: [u8; 3]) -> Image {
    let w: u32 = 34;
    let h: u32 = 130;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();
    for px in slice.iter_mut() {
        *px = Rgba8Pixel { r: 11, g: 14, b: 18, a: 255 };
    }

    let num_segs = 20usize;
    let lit_count = ((ratio.clamp(0.0, 1.0) * num_segs as f64).round() as usize).max(1);
    let seg_w = 22usize;
    let cx_start = (w as usize - seg_w) / 2;
    let step_y = (h as usize - 8) / num_segs;
    let seg_h = step_y.saturating_sub(2).max(3);

    for s in 0..num_segs {
        let y_top = h as usize - 4 - (s + 1) * step_y;
        let is_lit = s < lit_count;
        for y in y_top..(y_top + seg_h).min(h as usize) {
            for x in cx_start..(cx_start + seg_w).min(w as usize) {
                let px = &mut slice[y * w as usize + x];
                if is_lit {
                    *px = Rgba8Pixel {
                        r: col[0].saturating_add(25),
                        g: col[1].saturating_add(25),
                        b: col[2].saturating_add(25),
                        a: 255,
                    };
                } else {
                    *px = Rgba8Pixel {
                        r: col[0] / 8 + 10,
                        g: col[1] / 8 + 10,
                        b: col[2] / 8 + 14,
                        a: 255,
                    };
                }
            }
        }
    }

    Image::from_rgba8(buf)
}

/// Renders a horizontal glowing segmented LED bar (used on Summary bottom cards & Performance header).
fn render_horizontal_led_bar(
    w: u32,
    h: u32,
    num_segs: usize,
    ratio: f64,
    col: [u8; 3],
) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();
    for px in slice.iter_mut() {
        *px = Rgba8Pixel { r: 9, g: 12, b: 16, a: 255 };
    }

    let lit_count = ((ratio.clamp(0.0, 1.0) * num_segs as f64).round() as usize).max(1);
    let pad_x = 4usize;
    let pad_y = 3usize;
    let usable_w = (w as usize).saturating_sub(pad_x * 2);
    let step_x = (usable_w / num_segs.max(1)).max(3);
    let seg_w = step_x.saturating_sub(2).max(2);
    let seg_h = (h as usize).saturating_sub(pad_y * 2).max(4);

    for s in 0..num_segs {
        let x_left = pad_x + s * step_x;
        let is_lit = s < lit_count;

        if is_lit {
            for gy in pad_y.saturating_sub(2)..(pad_y + seg_h + 2).min(h as usize) {
                for gx in x_left.saturating_sub(2)..(x_left + seg_w + 2).min(w as usize) {
                    let p = &mut slice[gy * w as usize + gx];
                    p.r = p.r.saturating_add(col[0] / 7);
                    p.g = p.g.saturating_add(col[1] / 7);
                    p.b = p.b.saturating_add(col[2] / 7);
                }
            }
        }

        for y in pad_y..(pad_y + seg_h).min(h as usize) {
            for x in x_left..(x_left + seg_w).min(w as usize) {
                let is_edge =
                    x == x_left || x + 1 == x_left + seg_w || y == pad_y || y + 1 == pad_y + seg_h;
                let px = &mut slice[y * w as usize + x];
                if is_lit {
                    if is_edge {
                        *px = Rgba8Pixel {
                            r: col[0],
                            g: col[1],
                            b: col[2],
                            a: 255,
                        };
                    } else {
                        *px = Rgba8Pixel {
                            r: col[0].saturating_add(45),
                            g: col[1].saturating_add(45),
                            b: col[2].saturating_add(45),
                            a: 255,
                        };
                    }
                } else {
                    *px = Rgba8Pixel {
                        r: col[0] / 10 + 10,
                        g: col[1] / 10 + 12,
                        b: col[2] / 10 + 15,
                        a: 255,
                    };
                }
            }
        }
    }

    Image::from_rgba8(buf)
}

/// Renders the 3-series `CPU OVERVIEW` chart on Summary (Green Utilization, Orange Temp, Purple Kernel).
fn render_3series_cpu_overview_graph(
    w: u32,
    h: u32,
    util_q: &VecDeque<f64>,
    temp_q: &VecDeque<f64>,
    kern_q: &VecDeque<f64>,
    show_grid: bool,
) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();

    for px in slice.iter_mut() {
        *px = Rgba8Pixel { r: 7, g: 13, b: 11, a: 255 };
    }

    if show_grid {
        let rows = 8u32;
        let cols = 16u32;
        for r in 0..=rows {
            let y = ((r * (h - 1)) / rows) as usize;
            for x in 0..w as usize {
                slice[y * w as usize + x] = Rgba8Pixel { r: 16, g: 38, b: 26, a: 255 };
            }
        }
        for c in 0..=cols {
            let x = ((c * (w - 1)) / cols) as usize;
            for y in 0..h as usize {
                slice[y * w as usize + x] = Rgba8Pixel { r: 16, g: 38, b: 26, a: 255 };
            }
        }
    }

    draw_series_curve(slice, w, h, kern_q, 100.0, [184, 77, 255], false);
    draw_series_curve(slice, w, h, temp_q, 100.0, [245, 166, 35], false);
    draw_series_curve(slice, w, h, util_q, 100.0, [34, 229, 139], true);

    Image::from_rgba8(buf)
}

fn sync_history_selection(w: &MainWindow, c: &GuiController) {
    let Some((lo, hi)) = c.selection else {
        w.set_sum_sel_active(false);
        w.set_sum_top_procs_banner(SharedString::from("TOP 18 CPU PROCESSES"));
        w.set_sum_top_procs_banner_right(SharedString::new());
        w.set_sum_cpu_hint(SharedString::from("Drag on the chart to inspect a past time range"));
        let top_rows: Vec<ProcRow> = c
            .last_snap
            .processes
            .iter()
            .take(18)
            .enumerate()
            .map(|(i, p)| {
                let mut r = to_proc_row(p, None);
                if p.name.contains("spectacle") || p.name.contains("wine") && i == 5 {
                    r.highlight_kind = 2;
                } else if p.cpu_pct >= 0.05 && (3..=11).contains(&i) {
                    r.highlight_kind = 1;
                }
                r
            })
            .collect();
        w.set_top_processes(ModelRc::new(VecModel::from(top_rows)));
        return;
    };

    // Fractions may fall outside 0..1 when the range is older than the zoom
    // window; the chart clips the band and hides off-screen edges.
    let last = (c.history_window - 1) as f32;
    w.set_sum_sel_active(true);
    w.set_sum_sel_a(c.window_index_of(lo) as f32 / last);
    w.set_sum_sel_b(c.window_index_of(hi) as f32 / last);

    let (rows, frames, avg_cpu) = c.history_range_rows(lo, hi);
    let t0 = c.frame_time_ms(lo).map(format_local_hms).unwrap_or_default();
    let t1 = c.frame_time_ms(hi).map(format_local_hms).unwrap_or_default();
    let span = c.frame_time_ms(hi).zip(c.frame_time_ms(lo)).map_or(0, |(b, a)| (b - a) / 1000);
    w.set_sum_top_procs_banner(SharedString::from("HISTORY"));
    w.set_sum_top_procs_banner_right(SharedString::from(format!("{t0}-{t1}")));
    w.set_sum_sel_summary(SharedString::from(format!(
        "{} samples · avg CPU {:.1}%",
        frames, avg_cpu
    )));
    w.set_sum_cpu_hint(SharedString::from(format!(
        "Selected {t0} – {t1} ({}) · click the chart to clear",
        format_span_secs(span)
    )));
    w.set_top_processes(ModelRc::new(VecModel::from(rows)));
}

/// "45s", "3m 20s", "10 min" style durations.
fn format_span_secs(secs: u64) -> String {
    match secs {
        0..=99 => format!("{secs}s"),
        _ if secs.is_multiple_of(60) => format!("{} min", secs / 60),
        _ => format!("{}m {:02}s", secs / 60, secs % 60),
    }
}

/// Formats a Unix epoch (ms) as local wall-clock time "HH:MM:SS".
fn format_local_hms(epoch_ms: u64) -> String {
    let secs = (epoch_ms / 1000) as i64;
    #[cfg(unix)]
    {
        let t: libc::time_t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: both pointers are valid for the duration of the call.
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec);
        }
    }
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

/// Generates SVG path commands for a series mapped to a 1000x1000 viewport.
/// Returns (svg_path_commands, calculated_max_value).
fn generate_svg_stroke_path(data: &VecDeque<f64>, max_val: f64) -> (String, f64) {
    if data.is_empty() {
        return (String::new(), 1.0);
    }
    let n = data.len();
    let effective_max = if max_val <= 0.0 {
        let max_elem = data.iter().copied().fold(0.001f64, f64::max);
        if max_elem < 1.0 {
            1.0
        } else if max_elem < 10.0 {
            (max_elem * 1.2).ceil()
        } else {
            (max_elem * 1.15).ceil()
        }
    } else {
        max_val
    };

    let scale_x = 1000.0 / ((n - 1).max(1) as f64);
    let mut path = String::with_capacity(n * 16);

    for (i, &val) in data.iter().enumerate() {
        let x = ((i as f64) * scale_x).round();
        let norm_y = (val / effective_max).clamp(0.0, 1.0);
        let y = (1000.0 - norm_y * 1000.0).round();
        use std::fmt::Write;
        if i == 0 {
            let _ = write!(path, "M {:.0} {:.0}", x, y);
        } else {
            let _ = write!(path, " L {:.0} {:.0}", x, y);
        }
    }
    (path, effective_max)
}

/// Generates a closed SVG fill polygon from stroke commands down to the bottom (y=1000).
fn generate_svg_fill_path(stroke_path: &str) -> String {
    if stroke_path.is_empty() {
        return String::new();
    }
    format!("{} L 1000 1000 L 0 1000 Z", stroke_path)
}

/// Renders a single-series area chart (used for Memory, GPU, Disk, Network, and left-nav sparklines).
fn render_single_area_graph(
    w: u32,
    h: u32,
    series: &VecDeque<f64>,
    fixed_max: f64,
    line_col: [u8; 3],
    grid_col: [u8; 3],
    show_grid: bool,
) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();

    for px in slice.iter_mut() {
        *px = Rgba8Pixel {
            r: (grid_col[0] / 3).max(6),
            g: (grid_col[1] / 3).max(8),
            b: (grid_col[2] / 3).max(12),
            a: 255,
        };
    }

    let max_val = if fixed_max > 0.0 {
        fixed_max
    } else {
        series.iter().copied().fold(10.0_f64, f64::max) * 1.15
    };

    if show_grid {
        let rows = if h < 60 { 3u32 } else { 6u32 };
        let cols = if w < 100 { 4u32 } else { 14u32 };
        for r in 0..=rows {
            let y = ((r * (h - 1)) / rows) as usize;
            for x in 0..w as usize {
                slice[y * w as usize + x] = Rgba8Pixel {
                    r: grid_col[0],
                    g: grid_col[1],
                    b: grid_col[2],
                    a: 255,
                };
            }
        }
        for c in 0..=cols {
            let x = ((c * (w - 1)) / cols) as usize;
            for y in 0..h as usize {
                slice[y * w as usize + x] = Rgba8Pixel {
                    r: grid_col[0],
                    g: grid_col[1],
                    b: grid_col[2],
                    a: 255,
                };
            }
        }
    }

    draw_series_curve(slice, w, h, series, max_val, line_col, true);

    Image::from_rgba8(buf)
}

fn draw_series_curve(
    slice: &mut [Rgba8Pixel],
    w: u32,
    h: u32,
    series: &VecDeque<f64>,
    max_val: f64,
    col: [u8; 3],
    fill_under: bool,
) {
    if series.len() < 2 || w < 4 || h < 4 {
        return;
    }
    let n = series.len();
    let mut prev_x = 0usize;
    let mut prev_y = (h - 1) as usize;

    for (i, &val) in series.iter().enumerate() {
        let x = (i * (w as usize - 1)) / (n - 1).max(1);
        let norm = (val / max_val.max(1.0)).clamp(0.0, 1.0);
        let y = ((1.0 - norm) * (h as f64 - 3.0)).round() as usize + 1;

        if i > 0 {
            let x0 = prev_x.min(x);
            let x1 = prev_x.max(x);
            for cx in x0..=x1 {
                let t = if x1 > x0 {
                    (cx - x0) as f64 / (x1 - x0) as f64
                } else {
                    0.0
                };
                let cy = if x >= prev_x {
                    (prev_y as f64 + t * (y as f64 - prev_y as f64)).round() as usize
                } else {
                    (y as f64 + t * (prev_y as f64 - y as f64)).round() as usize
                }
                .clamp(0, h as usize - 1);

                if fill_under {
                    for fy in (cy + 1)..(h as usize - 1) {
                        let p = &mut slice[fy * w as usize + cx];
                        p.r = ((p.r as u16 * 4 + col[0] as u16) / 5) as u8;
                        p.g = ((p.g as u16 * 4 + col[1] as u16) / 5) as u8;
                        p.b = ((p.b as u16 * 4 + col[2] as u16) / 5) as u8;
                    }
                }

                for dy in -1i32..=1i32 {
                    let ly = (cy as i32 + dy).clamp(0, h as i32 - 1) as usize;
                    slice[ly * w as usize + cx] = Rgba8Pixel {
                        r: col[0],
                        g: col[1],
                        b: col[2],
                        a: 255,
                    };
                }
            }
        }
        prev_x = x;
        prev_y = y;
    }
}


/// Renders the glowing cyan ECG pulse + `PULSEDECK` logo for the bottom-left sidebar.
/// Renders a strip of unlit 5x7 dot-matrix character cells for the VFD/LCD
/// banners, at 2x so it stays crisp on HiDPI screens. The cell pitch matches
/// the banner font's advance (DejaVu Sans Mono 15px + 1.5px letter spacing).
fn render_lcd_ghost_cells() -> Image {
    const SCALE: f32 = 2.0;
    const PITCH: f32 = 10.5308; // logical px per character
    const CELL_W: f32 = 9.0; // glyph advance without letter spacing
    const CELL_H: f32 = 17.0;
    const CELLS: u32 = 200;
    let w = (CELLS as f32 * PITCH * SCALE).ceil() as u32;
    let h = (CELL_H * SCALE) as u32;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let stride = w as usize;
    let px = buf.make_mut_slice();

    let (cols, rows) = (5usize, 7usize);
    let dot_w = CELL_W * SCALE / cols as f32;
    let dot_h = CELL_H * SCALE / rows as f32;
    for cell in 0..CELLS {
        let x0 = cell as f32 * PITCH * SCALE;
        // Faint cell backing so each character slot reads as one LCD segment.
        for y in 0..h as usize {
            for x in (x0.round() as usize)..((x0 + CELL_W * SCALE).round() as usize).min(stride) {
                px[y * stride + x] = Rgba8Pixel { r: 54, g: 232, b: 255, a: 16 };
            }
        }
        for r in 0..rows {
            for c in 0..cols {
                let dx0 = (x0 + c as f32 * dot_w + dot_w * 0.18).round() as usize;
                let dx1 = (x0 + (c + 1) as f32 * dot_w - dot_w * 0.18).round() as usize;
                let dy0 = (r as f32 * dot_h + dot_h * 0.16).round() as usize;
                let dy1 = ((r + 1) as f32 * dot_h - dot_h * 0.16).round() as usize;
                for y in dy0..dy1.min(h as usize) {
                    for x in dx0..dx1.min(stride) {
                        px[y * stride + x] = Rgba8Pixel { r: 54, g: 232, b: 255, a: 44 };
                    }
                }
            }
        }
    }
    Image::from_rgba8(buf)
}

fn render_sidebar_logo() -> Image {
    let w: u32 = 176;
    let h: u32 = 48;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let slice = buf.make_mut_slice();

    for px in slice.iter_mut() {
        *px = Rgba8Pixel { r: 13, g: 16, b: 21, a: 255 };
    }

    // Draw glowing cyan ECG waveform on the left (x=8..58)
    let mid_y = 26i32;
    for x in 6..58usize {
        let dy = match x {
            20..=23 => -((x as i32 - 20) * 4),
            24..=28 => -12 + (x as i32 - 24) * 6,
            29..=32 => 12 - (x as i32 - 29) * 6,
            33..=36 => -12 + (x as i32 - 33) * 3,
            _ => 0,
        };
        let y = (mid_y + dy).clamp(4, h as i32 - 4) as usize;
        for gy in y.saturating_sub(2)..=(y + 2).min(h as usize - 1) {
            for gx in x.saturating_sub(1)..=(x + 1).min(w as usize - 1) {
                let dist = (gy as i32 - y as i32).abs() + (gx as i32 - x as i32).abs();
                let p = &mut slice[gy * w as usize + gx];
                if dist <= 1 {
                    *p = Rgba8Pixel { r: 80, g: 235, b: 255, a: 255 };
                } else {
                    p.r = p.r.saturating_add(15);
                    p.g = p.g.saturating_add(95);
                    p.b = p.b.saturating_add(140);
                }
            }
        }
    }

    // Draw "PULSEDECK" in 2x scaled pixel font with cyan neon glow
    draw_scaled_text(slice, w as usize, h as usize, 62, 19, "PULSEDECK", 1, [90, 235, 255]);

    Image::from_rgba8(buf)
}

fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110],
        '6' => [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        _ => [0; 7],
    }
}


fn draw_scaled_text(
    slice: &mut [Rgba8Pixel],
    w: usize,
    h: usize,
    x_start: usize,
    y_start: usize,
    text: &str,
    scale: usize,
    col: [u8; 3],
) {
    let mut cur_x = x_start;
    for ch in text.chars() {
        let rows = glyph_5x7(ch);
        for (ry, &bits) in rows.iter().enumerate() {
            for rx in 0..5usize {
                if (bits >> (4 - rx)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cur_x + rx * scale + sx;
                            let py = y_start + ry * scale + sy;
                            if px < w && py < h {
                                slice[py * w + px] = Rgba8Pixel {
                                    r: col[0],
                                    g: col[1],
                                    b: col[2],
                                    a: 255,
                                };
                            }
                        }
                    }
                }
            }
        }
        cur_x += 6 * scale;
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::model::ProcessEntry;

    /// Pushes a synthetic sample where `burner` (PID 999_999) uses `cpu` percent.
    fn push_sample(c: &mut GuiController, template: &ProcessEntry, cpu: f64) {
        let mut snap = c.last_snap.clone();
        snap.seq += 1;
        snap.timestamp_epoch_ms += 1000;
        let mut p = template.clone();
        p.pid = 999_999;
        p.name = String::from("burner");
        p.cpu_pct = cpu;
        p.mem_rss_bytes = 64 << 20;
        snap.processes = if cpu > 0.0 { vec![p] } else { Vec::new() };
        c.apply_snapshot(snap);
    }

    #[test]
    fn selected_range_reports_process_activity() {
        let mut c = GuiController::new(AppSettings::default());
        let t = c.last_snap.processes[0].clone();
        for i in 0..40 {
            push_sample(&mut c, &t, if (20..30).contains(&i) { 80.0 } else { 0.0 });
        }
        let latest = c.last_snap.seq;
        let busy_lo = latest - 19; // i = 20
        let busy_hi = latest - 10; // i = 29

        // Map those samples to screen fractions the way the chart draws them
        // (phase 1 = static chart), then select them back.
        for window in HISTORY_WINDOWS {
            c.history_window = window;
            let n = window as f64;
            let frac = |seq: u64| ((c.window_index_of(seq) - 1.0) / (n - 2.0)) as f32;
            c.select_history_range(frac(busy_lo), frac(busy_hi), 1.0);
            assert_eq!(c.selection, Some((busy_lo, busy_hi)), "zoom window {window}");
        }

        let (rows, frames, _) = c.history_range_rows(busy_lo, busy_hi);
        assert_eq!(frames, 10);
        assert_eq!(rows[0].name.as_str(), "burner");
        assert_eq!(rows[0].cpu_str.as_str(), "80.0%");
        assert_eq!(rows[0].gpu_str.as_str(), "80%"); // peak column

        // A range outside the busy window must not report the process.
        let (rows, _, _) = c.history_range_rows(latest - 5, latest);
        assert!(rows.iter().all(|r| r.name.as_str() != "burner"));
    }

    #[test]
    fn selection_expires_when_scrolled_out() {
        let mut c = GuiController::new(AppSettings::default());
        let t = c.last_snap.processes[0].clone();
        push_sample(&mut c, &t, 50.0);
        let seq = c.last_snap.seq;
        c.selection = Some((seq, seq));
        for _ in 0..LONG_HISTORY_LEN {
            push_sample(&mut c, &t, 0.0);
        }
        assert_eq!(c.selection, None);
    }

    #[test]
    fn span_formatting() {
        assert_eq!(format_span_secs(45), "45s");
        assert_eq!(format_span_secs(600), "10 min");
        assert_eq!(format_span_secs(200), "3m 20s");
    }
}
