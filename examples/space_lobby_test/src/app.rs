//! App shell for testing the SpaceLobbyScreen widget in isolation.
//!
//! Creates a minimal window with fake space/room data to exercise
//! the SpaceLobbyScreen rendering and interaction logic.

use std::collections::HashMap;
use makepad_widgets::*;

use crate::space_lobby::{SpaceRoomInfo, RoomState, SpaceLobbyScreenWidgetRefExt};

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(900, 700)
                window.title: "Space Lobby Test"
                pass.clear_color: #FFFFFF

                body +: {
                    space_lobby_screen := SpaceLobbyScreen {}
                }
            }
        }
    }
}

app_main!(App);

#[derive(Script, ScriptHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] initialized: bool,
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        // Order matters: base widgets first, then styles, then app widgets, then app UI.
        makepad_widgets::script_mod(vm);
        crate::styles::script_mod(vm);
        crate::icon_button::script_mod(vm);
        crate::avatar::script_mod(vm);
        crate::space_lobby::script_mod(vm);

        App::from_script_mod(vm, self::script_mod)
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, _actions: &Actions) {
        if !self.initialized {
            self.initialized = true;
            self.load_fake_data(cx);
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl App {
    /// Populate the SpaceLobbyScreen with fake hierarchical space/room data.
    fn load_fake_data(&mut self, cx: &mut Cx) {
        let mut cache: HashMap<String, Vec<SpaceRoomInfo>> = HashMap::new();

        // Root space children: 6 subspaces + 3 rooms
        cache.insert("root".to_string(), vec![
            // Subspace 1: "Engineering"
            SpaceRoomInfo {
                id: "!space_engineering:example.org".to_string(),
                name: "Engineering".to_string(),
                topic: Some("All engineering discussions and projects".to_string()),
                num_joined_members: 342,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            // Subspace 2: "Design"
            SpaceRoomInfo {
                id: "!space_design:example.org".to_string(),
                name: "Design".to_string(),
                topic: Some("UI/UX design team".to_string()),
                num_joined_members: 128,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            // Subspace 3: "Community"
            SpaceRoomInfo {
                id: "!space_community:example.org".to_string(),
                name: "Community".to_string(),
                topic: Some("Open source community hub".to_string()),
                num_joined_members: 1056,
                state: Some(RoomState::Joined),
                children_count: Some(7),
            },
            // Subspace 4: "Research & Science"
            SpaceRoomInfo {
                id: "!space_research:example.org".to_string(),
                name: "Research & Science".to_string(),
                topic: Some("Academic research, papers, and scientific computing".to_string()),
                num_joined_members: 276,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            // Subspace 5: "Gaming"
            SpaceRoomInfo {
                id: "!space_gaming:example.org".to_string(),
                name: "Gaming".to_string(),
                topic: Some("Game development and gaming community".to_string()),
                num_joined_members: 489,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            // Subspace 6: "Creative Arts"
            SpaceRoomInfo {
                id: "!space_creative:example.org".to_string(),
                name: "Creative Arts".to_string(),
                topic: Some("Music, writing, photography, and creative pursuits".to_string()),
                num_joined_members: 203,
                state: Some(RoomState::Invited),
                children_count: Some(4),
            },
            // Top-level room: "Announcements"
            SpaceRoomInfo {
                id: "!room_announcements:example.org".to_string(),
                name: "Announcements".to_string(),
                topic: Some("Official announcements and news".to_string()),
                num_joined_members: 890,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            // Top-level room: "General"
            SpaceRoomInfo {
                id: "!room_general:example.org".to_string(),
                name: "General".to_string(),
                topic: Some("General discussions, off-topic, watercooler chat".to_string()),
                num_joined_members: 654,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            // Top-level room: "Meta"
            SpaceRoomInfo {
                id: "!room_meta:example.org".to_string(),
                name: "Meta".to_string(),
                topic: Some("Discussion about this space itself, rules, and moderation".to_string()),
                num_joined_members: 234,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Engineering subspace children: 3 subspaces + 2 rooms
        cache.insert("!space_engineering:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_eng_rust:example.org".to_string(),
                name: "Rust Development".to_string(),
                topic: Some("Rust language discussions and help".to_string()),
                num_joined_members: 215,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_eng_web:example.org".to_string(),
                name: "Web Platform".to_string(),
                topic: Some("Frontend and backend web development".to_string()),
                num_joined_members: 178,
                state: Some(RoomState::Left),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!space_eng_infra:example.org".to_string(),
                name: "Infrastructure".to_string(),
                topic: Some("CI/CD, servers, deployment, DevOps".to_string()),
                num_joined_members: 95,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            SpaceRoomInfo {
                id: "!room_eng_mobile:example.org".to_string(),
                name: "Mobile Development".to_string(),
                topic: Some("iOS, Android, and cross-platform mobile dev".to_string()),
                num_joined_members: 67,
                state: None,
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_code_review:example.org".to_string(),
                name: "Code Review".to_string(),
                topic: Some("Share PRs and get code reviews from peers".to_string()),
                num_joined_members: 143,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Rust Development subspace (depth 2): 2 subspaces + 2 rooms
        cache.insert("!space_eng_rust:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_eng_rust_async:example.org".to_string(),
                name: "Async & Concurrency".to_string(),
                topic: Some("Tokio, async-std, threads, and actors".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!space_eng_rust_wasm:example.org".to_string(),
                name: "Rust + WASM".to_string(),
                topic: Some("WebAssembly targets, wasm-bindgen, wasm-pack".to_string()),
                num_joined_members: 72,
                state: Some(RoomState::Joined),
                children_count: Some(2),
            },
            SpaceRoomInfo {
                id: "!room_eng_rust_beginners:example.org".to_string(),
                name: "Beginners".to_string(),
                topic: Some("New to Rust? Ask questions here".to_string()),
                num_joined_members: 189,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_rust_libs:example.org".to_string(),
                name: "Libraries & Crates".to_string(),
                topic: Some("Discuss crates, publish your own, find dependencies".to_string()),
                num_joined_members: 156,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Async & Concurrency subspace (depth 3): 3 rooms
        cache.insert("!space_eng_rust_async:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_eng_rust_async_tokio:example.org".to_string(),
                name: "Tokio".to_string(),
                topic: Some("Tokio runtime, tasks, and I/O".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_rust_async_channels:example.org".to_string(),
                name: "Channels & Actors".to_string(),
                topic: Some("Message passing, crossbeam, and actor patterns".to_string()),
                num_joined_members: 34,
                state: None,
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_rust_async_debugging:example.org".to_string(),
                name: "Async Debugging".to_string(),
                topic: Some("Debugging async code, tracing, and diagnostics".to_string()),
                num_joined_members: 29,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Rust + WASM subspace (depth 3): 2 rooms
        cache.insert("!space_eng_rust_wasm:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_eng_rust_wasm_bindgen:example.org".to_string(),
                name: "wasm-bindgen".to_string(),
                topic: Some("JS interop, wasm-bindgen, and web-sys".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_rust_wasm_deploy:example.org".to_string(),
                name: "WASM Deployment".to_string(),
                topic: Some("Building and deploying WASM modules".to_string()),
                num_joined_members: 31,
                state: Some(RoomState::Invited),
                children_count: None,
            },
        ]);

        // Web Platform subspace (depth 2): 1 subspace + 2 rooms
        cache.insert("!space_eng_web:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_eng_web_frontend:example.org".to_string(),
                name: "Frontend Frameworks".to_string(),
                topic: Some("React, Vue, Svelte, and beyond".to_string()),
                num_joined_members: 134,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!room_eng_web_api:example.org".to_string(),
                name: "API Design".to_string(),
                topic: Some("REST, GraphQL, gRPC, and API best practices".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_web_perf:example.org".to_string(),
                name: "Web Performance".to_string(),
                topic: Some("Core Web Vitals, Lighthouse, and optimization".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Frontend Frameworks subspace (depth 3): 4 rooms
        cache.insert("!space_eng_web_frontend:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_eng_web_frontend_react:example.org".to_string(),
                name: "React".to_string(),
                topic: Some("React, Next.js, and the React ecosystem".to_string()),
                num_joined_members: 87,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_web_frontend_vue:example.org".to_string(),
                name: "Vue".to_string(),
                topic: Some("Vue 3, Nuxt, Pinia, and the Vue ecosystem".to_string()),
                num_joined_members: 52,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_web_frontend_svelte:example.org".to_string(),
                name: "Svelte".to_string(),
                topic: Some("Svelte, SvelteKit, and runes".to_string()),
                num_joined_members: 41,
                state: None,
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_web_frontend_css:example.org".to_string(),
                name: "CSS & Styling".to_string(),
                topic: Some("Tailwind, CSS modules, and modern CSS features".to_string()),
                num_joined_members: 63,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Infrastructure subspace (depth 2): 2 subspaces + 3 rooms
        cache.insert("!space_eng_infra:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_eng_infra_k8s:example.org".to_string(),
                name: "Kubernetes".to_string(),
                topic: Some("Container orchestration and cluster management".to_string()),
                num_joined_members: 78,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!space_eng_infra_ci:example.org".to_string(),
                name: "CI/CD Pipelines".to_string(),
                topic: Some("GitHub Actions, Jenkins, GitLab CI".to_string()),
                num_joined_members: 65,
                state: Some(RoomState::Joined),
                children_count: Some(2),
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_monitoring:example.org".to_string(),
                name: "Monitoring & Alerts".to_string(),
                topic: Some("Prometheus, Grafana, PagerDuty, and alerting".to_string()),
                num_joined_members: 43,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_security:example.org".to_string(),
                name: "Security".to_string(),
                topic: Some("Vulnerability scanning, secrets management, and auditing".to_string()),
                num_joined_members: 38,
                state: Some(RoomState::Invited),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_networking:example.org".to_string(),
                name: "Networking".to_string(),
                topic: Some("DNS, load balancers, CDNs, and network policies".to_string()),
                num_joined_members: 29,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Kubernetes subspace (depth 3): 3 rooms
        cache.insert("!space_eng_infra_k8s:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_eng_infra_k8s_deploy:example.org".to_string(),
                name: "Deployments & Helm".to_string(),
                topic: Some("Helm charts, kustomize, and deployment strategies".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_k8s_debug:example.org".to_string(),
                name: "Debugging K8s".to_string(),
                topic: Some("kubectl tips, pod debugging, and log aggregation".to_string()),
                num_joined_members: 32,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_k8s_operators:example.org".to_string(),
                name: "Operators & CRDs".to_string(),
                topic: Some("Custom controllers and operator patterns".to_string()),
                num_joined_members: 18,
                state: None,
                children_count: None,
            },
        ]);

        // CI/CD Pipelines subspace (depth 3): 2 rooms
        cache.insert("!space_eng_infra_ci:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_eng_infra_ci_gh:example.org".to_string(),
                name: "GitHub Actions".to_string(),
                topic: Some("Workflows, reusable actions, and runners".to_string()),
                num_joined_members: 54,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_eng_infra_ci_testing:example.org".to_string(),
                name: "Test Automation".to_string(),
                topic: Some("Integration tests, E2E tests, and test infrastructure".to_string()),
                num_joined_members: 37,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Design subspace children: 3 subspaces + 2 rooms
        cache.insert("!space_design:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_design_ui:example.org".to_string(),
                name: "UI Components".to_string(),
                topic: Some("Shared UI component library and patterns".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_design_ux:example.org".to_string(),
                name: "UX Research".to_string(),
                topic: Some("User experience research and findings".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Invited),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!space_design_visual:example.org".to_string(),
                name: "Visual Design".to_string(),
                topic: Some("Graphics, illustration, and visual language".to_string()),
                num_joined_members: 73,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            SpaceRoomInfo {
                id: "!room_design_brand:example.org".to_string(),
                name: "Brand & Identity".to_string(),
                topic: Some("Logos, colors, typography, and brand guidelines".to_string()),
                num_joined_members: 32,
                state: Some(RoomState::Left),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_assets:example.org".to_string(),
                name: "Design Assets".to_string(),
                topic: Some("Shared design files, mockups, and prototypes".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // UI Components subspace (depth 2): 1 subspace + 3 rooms
        cache.insert("!space_design_ui:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_design_ui_mobile:example.org".to_string(),
                name: "Mobile Components".to_string(),
                topic: Some("Mobile-specific UI patterns and components".to_string()),
                num_joined_members: 41,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_design_ui_buttons:example.org".to_string(),
                name: "Buttons & Controls".to_string(),
                topic: Some("Button styles, toggles, switches, and interactive controls".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ui_forms:example.org".to_string(),
                name: "Forms & Input".to_string(),
                topic: Some("Text fields, dropdowns, date pickers, and form patterns".to_string()),
                num_joined_members: 48,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ui_nav:example.org".to_string(),
                name: "Navigation".to_string(),
                topic: Some("Nav bars, tabs, sidebars, and breadcrumbs".to_string()),
                num_joined_members: 37,
                state: None,
                children_count: None,
            },
        ]);

        // Mobile Components subspace (depth 3): 3 rooms
        cache.insert("!space_design_ui_mobile:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_design_ui_mobile_ios:example.org".to_string(),
                name: "iOS Patterns".to_string(),
                topic: Some("iOS HIG-compliant components and patterns".to_string()),
                num_joined_members: 23,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ui_mobile_android:example.org".to_string(),
                name: "Material Design".to_string(),
                topic: Some("Material 3 components and Android patterns".to_string()),
                num_joined_members: 19,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ui_mobile_responsive:example.org".to_string(),
                name: "Responsive Layout".to_string(),
                topic: Some("Adaptive layouts for different screen sizes".to_string()),
                num_joined_members: 31,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // UX Research subspace (depth 2): 3 rooms
        cache.insert("!space_design_ux:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_design_ux_testing:example.org".to_string(),
                name: "Usability Testing".to_string(),
                topic: Some("Planning and conducting usability tests".to_string()),
                num_joined_members: 28,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ux_analytics:example.org".to_string(),
                name: "Analytics & Metrics".to_string(),
                topic: Some("User behavior analytics and UX metrics".to_string()),
                num_joined_members: 22,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_ux_accessibility:example.org".to_string(),
                name: "Accessibility".to_string(),
                topic: Some("WCAG compliance, screen readers, and inclusive design".to_string()),
                num_joined_members: 35,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Visual Design subspace (depth 2): 2 subspaces + 3 rooms
        cache.insert("!space_design_visual:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_design_visual_icons:example.org".to_string(),
                name: "Iconography".to_string(),
                topic: Some("Icon systems, icon fonts, and SVG icons".to_string()),
                num_joined_members: 33,
                state: Some(RoomState::Joined),
                children_count: Some(2),
            },
            SpaceRoomInfo {
                id: "!space_design_visual_color:example.org".to_string(),
                name: "Color Systems".to_string(),
                topic: Some("Color palettes, themes, and dynamic color".to_string()),
                num_joined_members: 27,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_design_visual_typo:example.org".to_string(),
                name: "Typography".to_string(),
                topic: Some("Font selection, type scales, and text styling".to_string()),
                num_joined_members: 41,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_visual_motion:example.org".to_string(),
                name: "Motion & Animation".to_string(),
                topic: Some("Transitions, animations, and micro-interactions".to_string()),
                num_joined_members: 38,
                state: None,
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_visual_layout:example.org".to_string(),
                name: "Layout & Grid".to_string(),
                topic: Some("Grid systems, spacing scales, and page layout".to_string()),
                num_joined_members: 29,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Iconography subspace (depth 3): 2 rooms
        cache.insert("!space_design_visual_icons:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_design_visual_icons_svg:example.org".to_string(),
                name: "SVG Icons".to_string(),
                topic: Some("Creating and optimizing SVG icon sets".to_string()),
                num_joined_members: 18,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_visual_icons_emoji:example.org".to_string(),
                name: "Custom Emoji".to_string(),
                topic: Some("Custom emoji packs and sticker design".to_string()),
                num_joined_members: 24,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Color Systems subspace (depth 3): 3 rooms
        cache.insert("!space_design_visual_color:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_design_visual_color_dark:example.org".to_string(),
                name: "Dark Mode".to_string(),
                topic: Some("Dark theme design, contrast, and readability".to_string()),
                num_joined_members: 21,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_visual_color_tokens:example.org".to_string(),
                name: "Design Tokens".to_string(),
                topic: Some("Color tokens, semantic naming, and token systems".to_string()),
                num_joined_members: 16,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_design_visual_color_contrast:example.org".to_string(),
                name: "Contrast & A11y".to_string(),
                topic: Some("Color contrast ratios and accessibility compliance".to_string()),
                num_joined_members: 14,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Community subspace children: 3 subspaces + 4 rooms
        cache.insert("!space_community:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_community_events:example.org".to_string(),
                name: "Events & Meetups".to_string(),
                topic: Some("Community events coordination".to_string()),
                num_joined_members: 234,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_community_education:example.org".to_string(),
                name: "Education & Learning".to_string(),
                topic: Some("Tutorials, courses, and learning resources".to_string()),
                num_joined_members: 312,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            SpaceRoomInfo {
                id: "!space_community_contrib:example.org".to_string(),
                name: "Open Source Contrib".to_string(),
                topic: Some("Contributing to open source projects".to_string()),
                num_joined_members: 187,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!room_community_intro:example.org".to_string(),
                name: "Introductions".to_string(),
                topic: Some("Introduce yourself to the community!".to_string()),
                num_joined_members: 567,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_help:example.org".to_string(),
                name: "Help & Support".to_string(),
                topic: Some("Ask for help, get answers from the community".to_string()),
                num_joined_members: 432,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_offtopic:example.org".to_string(),
                name: "Off Topic".to_string(),
                topic: Some("Random conversations, memes, and fun".to_string()),
                num_joined_members: 678,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_feedback:example.org".to_string(),
                name: "Feedback".to_string(),
                topic: Some("Submit feature requests and bug reports".to_string()),
                num_joined_members: 156,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Events & Meetups subspace (depth 2): 1 subspace + 3 rooms
        cache.insert("!space_community_events:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_community_events_conf:example.org".to_string(),
                name: "Conferences".to_string(),
                topic: Some("Conference talks, CFPs, and conference discussion".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_events_upcoming:example.org".to_string(),
                name: "Upcoming Events".to_string(),
                topic: Some("Schedule of upcoming community events".to_string()),
                num_joined_members: 198,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_events_planning:example.org".to_string(),
                name: "Event Planning".to_string(),
                topic: Some("Organize and coordinate community events".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_events_recordings:example.org".to_string(),
                name: "Recordings & Notes".to_string(),
                topic: Some("Past event recordings and meeting notes".to_string()),
                num_joined_members: 134,
                state: None,
                children_count: None,
            },
        ]);

        // Conferences subspace (depth 3): 3 rooms
        cache.insert("!space_community_events_conf:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_events_conf_rustconf:example.org".to_string(),
                name: "RustConf".to_string(),
                topic: Some("RustConf discussion, planning, and hallway track".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_events_conf_fosdem:example.org".to_string(),
                name: "FOSDEM".to_string(),
                topic: Some("FOSDEM talks, devrooms, and coordination".to_string()),
                num_joined_members: 43,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_events_conf_cfp:example.org".to_string(),
                name: "Call for Papers".to_string(),
                topic: Some("CFP announcements and talk proposal help".to_string()),
                num_joined_members: 29,
                state: Some(RoomState::Invited),
                children_count: None,
            },
        ]);

        // Education & Learning subspace (depth 2): 2 subspaces + 3 rooms
        cache.insert("!space_community_education:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_community_edu_rust:example.org".to_string(),
                name: "Learn Rust".to_string(),
                topic: Some("Rust learning path, exercises, and mentoring".to_string()),
                num_joined_members: 189,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_community_edu_webdev:example.org".to_string(),
                name: "Learn Web Dev".to_string(),
                topic: Some("Web development tutorials and learning".to_string()),
                num_joined_members: 156,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_community_edu_books:example.org".to_string(),
                name: "Book Club".to_string(),
                topic: Some("Monthly technical book discussions".to_string()),
                num_joined_members: 87,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_edu_videos:example.org".to_string(),
                name: "Video Tutorials".to_string(),
                topic: Some("Share and discuss video learning resources".to_string()),
                num_joined_members: 112,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_edu_mentoring:example.org".to_string(),
                name: "Mentoring".to_string(),
                topic: Some("Find a mentor or become one".to_string()),
                num_joined_members: 64,
                state: None,
                children_count: None,
            },
        ]);

        // Learn Rust subspace (depth 3): 4 rooms
        cache.insert("!space_community_edu_rust:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_edu_rust_beginner:example.org".to_string(),
                name: "Beginner Exercises".to_string(),
                topic: Some("Rustlings, simple projects, and getting started".to_string()),
                num_joined_members: 134,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_edu_rust_intermediate:example.org".to_string(),
                name: "Intermediate Rust".to_string(),
                topic: Some("Traits, generics, lifetimes, and error handling".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_edu_rust_advanced:example.org".to_string(),
                name: "Advanced Topics".to_string(),
                topic: Some("Unsafe, proc macros, compiler internals, and deep dives".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_edu_rust_projects:example.org".to_string(),
                name: "Project Ideas".to_string(),
                topic: Some("Project ideas and build-along sessions".to_string()),
                num_joined_members: 78,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Learn Web Dev subspace (depth 3): 3 rooms
        cache.insert("!space_community_edu_webdev:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_edu_webdev_html:example.org".to_string(),
                name: "HTML & CSS Basics".to_string(),
                topic: Some("Fundamentals of HTML, CSS, and web layout".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_edu_webdev_js:example.org".to_string(),
                name: "JavaScript".to_string(),
                topic: Some("JavaScript fundamentals and modern ES features".to_string()),
                num_joined_members: 112,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_edu_webdev_fullstack:example.org".to_string(),
                name: "Full Stack".to_string(),
                topic: Some("Building full stack apps from scratch".to_string()),
                num_joined_members: 76,
                state: Some(RoomState::Invited),
                children_count: None,
            },
        ]);

        // Open Source Contrib subspace (depth 2): 1 subspace + 3 rooms
        cache.insert("!space_community_contrib:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_community_contrib_goodfirst:example.org".to_string(),
                name: "Good First Issues".to_string(),
                topic: Some("Curated beginner-friendly contribution opportunities".to_string()),
                num_joined_members: 145,
                state: Some(RoomState::Joined),
                children_count: Some(2),
            },
            SpaceRoomInfo {
                id: "!room_community_contrib_review:example.org".to_string(),
                name: "PR Reviews".to_string(),
                topic: Some("Get reviews on your open source PRs".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_contrib_maintainer:example.org".to_string(),
                name: "Maintainer Chat".to_string(),
                topic: Some("Discussion for OSS maintainers".to_string()),
                num_joined_members: 43,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_community_contrib_hacktober:example.org".to_string(),
                name: "Hacktoberfest".to_string(),
                topic: Some("Annual Hacktoberfest coordination and tracking".to_string()),
                num_joined_members: 234,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Good First Issues subspace (depth 3): 2 rooms
        cache.insert("!space_community_contrib_goodfirst:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_contrib_gfi_rust:example.org".to_string(),
                name: "Rust Projects".to_string(),
                topic: Some("Good first issues in Rust open source projects".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_contrib_gfi_docs:example.org".to_string(),
                name: "Documentation".to_string(),
                topic: Some("Docs improvements and translation contributions".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Research & Science subspace: 2 subspaces + 2 rooms
        cache.insert("!space_research:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_research_ml:example.org".to_string(),
                name: "Machine Learning".to_string(),
                topic: Some("ML models, training, and inference".to_string()),
                num_joined_members: 167,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_research_data:example.org".to_string(),
                name: "Data Science".to_string(),
                topic: Some("Data analysis, visualization, and statistics".to_string()),
                num_joined_members: 134,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_research_papers:example.org".to_string(),
                name: "Paper Reading".to_string(),
                topic: Some("Weekly paper reading group and discussions".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_research_hpc:example.org".to_string(),
                name: "HPC & Clusters".to_string(),
                topic: Some("High-performance computing and cluster management".to_string()),
                num_joined_members: 45,
                state: None,
                children_count: None,
            },
        ]);

        // Machine Learning subspace (depth 2): 1 subspace + 3 rooms
        cache.insert("!space_research_ml:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_research_ml_llm:example.org".to_string(),
                name: "LLMs & NLP".to_string(),
                topic: Some("Large language models and natural language processing".to_string()),
                num_joined_members: 112,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_research_ml_vision:example.org".to_string(),
                name: "Computer Vision".to_string(),
                topic: Some("Image recognition, object detection, and video analysis".to_string()),
                num_joined_members: 78,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_research_ml_training:example.org".to_string(),
                name: "Training Infra".to_string(),
                topic: Some("GPU clusters, distributed training, and MLOps".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_research_ml_ethics:example.org".to_string(),
                name: "AI Ethics".to_string(),
                topic: Some("Responsible AI, bias, fairness, and safety".to_string()),
                num_joined_members: 94,
                state: Some(RoomState::Invited),
                children_count: None,
            },
        ]);

        // LLMs & NLP subspace (depth 3): 3 rooms
        cache.insert("!space_research_ml_llm:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_ml_llm_finetuning:example.org".to_string(),
                name: "Fine-tuning".to_string(),
                topic: Some("LoRA, QLoRA, PEFT, and fine-tuning techniques".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_ml_llm_prompting:example.org".to_string(),
                name: "Prompt Engineering".to_string(),
                topic: Some("Prompt design, chain-of-thought, and evaluation".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_ml_llm_rag:example.org".to_string(),
                name: "RAG & Retrieval".to_string(),
                topic: Some("Retrieval-augmented generation and vector databases".to_string()),
                num_joined_members: 54,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Data Science subspace (depth 2): 3 rooms
        cache.insert("!space_research_data:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_data_viz:example.org".to_string(),
                name: "Visualization".to_string(),
                topic: Some("D3, Plotly, matplotlib, and data viz best practices".to_string()),
                num_joined_members: 78,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_data_pipelines:example.org".to_string(),
                name: "Data Pipelines".to_string(),
                topic: Some("ETL, streaming, and data engineering".to_string()),
                num_joined_members: 65,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_data_stats:example.org".to_string(),
                name: "Statistics".to_string(),
                topic: Some("Statistical methods, hypothesis testing, and Bayesian analysis".to_string()),
                num_joined_members: 43,
                state: None,
                children_count: None,
            },
        ]);

        // Gaming subspace: 2 subspaces + 3 rooms
        cache.insert("!space_gaming:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_gaming_dev:example.org".to_string(),
                name: "Game Development".to_string(),
                topic: Some("Game engines, game design, and game programming".to_string()),
                num_joined_members: 234,
                state: Some(RoomState::Joined),
                children_count: Some(5),
            },
            SpaceRoomInfo {
                id: "!space_gaming_esports:example.org".to_string(),
                name: "Esports".to_string(),
                topic: Some("Competitive gaming and tournament organization".to_string()),
                num_joined_members: 178,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_gaming_retro:example.org".to_string(),
                name: "Retro Gaming".to_string(),
                topic: Some("Classic games, emulation, and retro hardware".to_string()),
                num_joined_members: 145,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_gaming_indie:example.org".to_string(),
                name: "Indie Games".to_string(),
                topic: Some("Indie game discovery and discussion".to_string()),
                num_joined_members: 198,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_gaming_lfg:example.org".to_string(),
                name: "Looking for Group".to_string(),
                topic: Some("Find players for multiplayer games".to_string()),
                num_joined_members: 312,
                state: None,
                children_count: None,
            },
        ]);

        // Game Development subspace (depth 2): 2 subspaces + 3 rooms
        cache.insert("!space_gaming_dev:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_gaming_dev_engines:example.org".to_string(),
                name: "Game Engines".to_string(),
                topic: Some("Unity, Unreal, Godot, Bevy, and custom engines".to_string()),
                num_joined_members: 156,
                state: Some(RoomState::Joined),
                children_count: Some(4),
            },
            SpaceRoomInfo {
                id: "!space_gaming_dev_art:example.org".to_string(),
                name: "Game Art".to_string(),
                topic: Some("2D art, 3D modeling, animation, and shaders".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_gaming_dev_audio:example.org".to_string(),
                name: "Game Audio".to_string(),
                topic: Some("Sound design, music, and audio programming".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Invited),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_gaming_dev_design:example.org".to_string(),
                name: "Game Design".to_string(),
                topic: Some("Mechanics, balancing, level design, and narratives".to_string()),
                num_joined_members: 123,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_gaming_dev_jams:example.org".to_string(),
                name: "Game Jams".to_string(),
                topic: Some("Ludum Dare, GMTK, and game jam coordination".to_string()),
                num_joined_members: 89,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Game Engines subspace (depth 3): 4 rooms
        cache.insert("!space_gaming_dev_engines:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_engines_bevy:example.org".to_string(),
                name: "Bevy".to_string(),
                topic: Some("Bevy game engine in Rust - ECS, rendering, and plugins".to_string()),
                num_joined_members: 87,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_engines_godot:example.org".to_string(),
                name: "Godot".to_string(),
                topic: Some("Godot 4, GDScript, GDExtension, and Godot-Rust".to_string()),
                num_joined_members: 112,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_engines_unity:example.org".to_string(),
                name: "Unity".to_string(),
                topic: Some("Unity engine, C#, DOTS, and Unity packages".to_string()),
                num_joined_members: 134,
                state: Some(RoomState::Left),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_engines_custom:example.org".to_string(),
                name: "Custom Engines".to_string(),
                topic: Some("Building your own game engine from scratch".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Game Art subspace (depth 3): 3 rooms
        cache.insert("!space_gaming_dev_art:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_art_pixel:example.org".to_string(),
                name: "Pixel Art".to_string(),
                topic: Some("Pixel art techniques, tools, and sprite creation".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_art_3d:example.org".to_string(),
                name: "3D Modeling".to_string(),
                topic: Some("Blender, Maya, ZBrush, and 3D asset creation".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_art_shaders:example.org".to_string(),
                name: "Shaders & VFX".to_string(),
                topic: Some("Shader programming, VFX, and post-processing".to_string()),
                num_joined_members: 43,
                state: None,
                children_count: None,
            },
        ]);

        // Esports subspace (depth 2): 3 rooms
        cache.insert("!space_gaming_esports:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_esports_tournaments:example.org".to_string(),
                name: "Tournaments".to_string(),
                topic: Some("Tournament brackets, schedules, and results".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_esports_coaching:example.org".to_string(),
                name: "Coaching".to_string(),
                topic: Some("Get coaching and improve your competitive play".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_esports_streaming:example.org".to_string(),
                name: "Streaming".to_string(),
                topic: Some("Stream setup, OBS, Twitch, and content creation".to_string()),
                num_joined_members: 67,
                state: Some(RoomState::Invited),
                children_count: None,
            },
        ]);

        // Creative Arts subspace: 2 subspaces + 2 rooms
        cache.insert("!space_creative:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!space_creative_music:example.org".to_string(),
                name: "Music Production".to_string(),
                topic: Some("DAWs, synthesis, mixing, and music theory".to_string()),
                num_joined_members: 98,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!space_creative_writing:example.org".to_string(),
                name: "Writing".to_string(),
                topic: Some("Fiction, non-fiction, technical writing, and blogging".to_string()),
                num_joined_members: 87,
                state: Some(RoomState::Joined),
                children_count: Some(3),
            },
            SpaceRoomInfo {
                id: "!room_creative_photo:example.org".to_string(),
                name: "Photography".to_string(),
                topic: Some("Camera gear, editing, and photo sharing".to_string()),
                num_joined_members: 76,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_creative_video:example.org".to_string(),
                name: "Video Production".to_string(),
                topic: Some("Video editing, cinematography, and YouTube".to_string()),
                num_joined_members: 54,
                state: None,
                children_count: None,
            },
        ]);

        // Music Production subspace (depth 2): 3 rooms
        cache.insert("!space_creative_music:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_music_synth:example.org".to_string(),
                name: "Synthesis".to_string(),
                topic: Some("Synthesizers, sound design, and modular".to_string()),
                num_joined_members: 45,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_music_mixing:example.org".to_string(),
                name: "Mixing & Mastering".to_string(),
                topic: Some("EQ, compression, and mastering techniques".to_string()),
                num_joined_members: 38,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_music_collab:example.org".to_string(),
                name: "Collaborations".to_string(),
                topic: Some("Find collaborators and share works in progress".to_string()),
                num_joined_members: 29,
                state: Some(RoomState::Joined),
                children_count: None,
            },
        ]);

        // Writing subspace (depth 2): 3 rooms
        cache.insert("!space_creative_writing:example.org".to_string(), vec![
            SpaceRoomInfo {
                id: "!room_writing_fiction:example.org".to_string(),
                name: "Fiction".to_string(),
                topic: Some("Short stories, novels, and worldbuilding".to_string()),
                num_joined_members: 43,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_writing_tech:example.org".to_string(),
                name: "Technical Writing".to_string(),
                topic: Some("Documentation, blog posts, and dev articles".to_string()),
                num_joined_members: 56,
                state: Some(RoomState::Joined),
                children_count: None,
            },
            SpaceRoomInfo {
                id: "!room_writing_feedback:example.org".to_string(),
                name: "Writing Feedback".to_string(),
                topic: Some("Share drafts and get constructive feedback".to_string()),
                num_joined_members: 34,
                state: Some(RoomState::Left),
                children_count: None,
            },
        ]);

        // Load the data into the SpaceLobbyScreen
        let space_lobby = self.ui.space_lobby_screen(cx, ids!(space_lobby_screen));
        space_lobby.set_displayed_space(cx, "Makepad Community", cache);
    }
}
