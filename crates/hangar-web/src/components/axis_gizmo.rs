//! Blender-style axis gizmo over the viewer: the world axes (glTF: +Y up, models face +Z) as the
//! camera sees them. Clicking an axis end turns the view to look from it.

use crate::viewer::ViewAngles;
use bevy::math::{EulerRot, Quat, Vec3};
use leptos::prelude::*;

/// Distance from the centre to the axis ends, in px (the gizmo is 96 px wide).
const REACH: f32 = 33.0;

struct End {
    dir: Vec3,
    label: &'static str,
    class: &'static str,
    title: &'static str,
}

const ENDS: [End; 6] = [
    End { dir: Vec3::X, label: "X", class: "axis-x", title: "Ver de +X" },
    End { dir: Vec3::Y, label: "Y", class: "axis-y", title: "Ver de cima (+Y)" },
    End { dir: Vec3::Z, label: "Z", class: "axis-z", title: "Ver de frente (+Z, a frente no glTF)" },
    End { dir: Vec3::NEG_X, label: "−X", class: "axis-x neg", title: "Ver de −X" },
    End { dir: Vec3::NEG_Y, label: "−Y", class: "axis-y neg", title: "Ver de baixo (−Y)" },
    End { dir: Vec3::NEG_Z, label: "−Z", class: "axis-z neg", title: "Ver de trás (−Z)" },
];

#[component]
pub fn AxisGizmo(
    /// Latest camera angles from the viewer (None until the first frame).
    view: Signal<Option<ViewAngles>>,
    /// An axis end was clicked: look from that direction.
    #[prop(into)]
    on_pick: Callback<Vec3>,
) -> impl IntoView {
    // World → camera space; x right, y up, z towards the viewer.
    let to_camera = Memo::new(move |_| {
        let v = view.get().unwrap_or(ViewAngles { yaw: 0.6, pitch: 0.35 });
        Quat::from_euler(EulerRot::YXZ, v.yaw, -v.pitch, 0.0).inverse()
    });
    let screen = move |dir: Vec3| {
        let p = to_camera.get() * dir;
        (p.x * REACH, -p.y * REACH, p.z)
    };

    let lines = ENDS[..3]
        .iter()
        .map(|e| {
            let dir = e.dir;
            view! {
                <line
                    class=e.class
                    x1="0"
                    y1="0"
                    x2=move || screen(dir).0
                    y2=move || screen(dir).1
                />
            }
        })
        .collect_view();
    let dots = ENDS
        .iter()
        .map(|e| {
            let dir = e.dir;
            view! {
                <button
                    class=format!("axis-dot {}", e.class)
                    title=e.title
                    aria-label=e.title
                    style:transform=move || {
                        let (x, y, _) = screen(dir);
                        format!("translate({x:.1}px, {y:.1}px)")
                    }
                    // Nearer ends on top, like Blender's depth-sorted circles.
                    style:z-index=move || format!("{}", (screen(dir).2 * 100.0).round() as i32 + 200)
                    on:click=move |_| on_pick.run(dir)
                >
                    <span>{e.label}</span>
                </button>
            }
        })
        .collect_view();

    view! {
        <div class="axis-gizmo">
            <svg viewBox="-48 -48 96 96" aria-hidden="true">
                {lines}
            </svg>
            {dots}
        </div>
    }
}
