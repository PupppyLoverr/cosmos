//! Computer Use — Linux semantic-observation probe / regression harness.
//!
//! Exercises the SAME `atspi` crate the Appshot semantic layer uses
//! (`crates/ui/src/appshots/linux/atspi.rs`) against the live AT-SPI registry:
//! enumerates applications, discovers top-level windows with roles/titles, and
//! traverses one window into a bounded semantic tree — reporting per-phase
//! latency. It is display-server aware and prints an honest capability verdict
//! (Wayland does NOT disable AT-SPI itself; Appshots refuse unique-window
//! matching on Wayland, and Cua background input is often unavailable).
//!
//! Usage: cargo run -p zeron-ui --example atspi-probe [--traverse "<window title>"]
//! Exit 0 = AT-SPI reachable and at least one application enumerated.

fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
}

fn main() {
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("atspi-probe is Linux-only");
        std::process::exit(2);
    }
    #[cfg(target_os = "linux")]
    if let Err(e) = linux::main() {
        eprintln!("FAIL: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::wayland;
    use anyhow::Context;
    use atspi::proxy::text::TextProxy;
    use atspi::{AccessibilityConnection, Interface, Role, State};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    const MAX_DEPTH: usize = 24;
    const MAX_NODES: usize = 800;
    const DEADLINE: Duration = Duration::from_millis(1500);

    pub fn main() -> anyhow::Result<()> {
        let want_title = std::env::args().skip_while(|a| a != "--traverse").nth(1);
        let display = if wayland() { "Wayland" } else { "X11" };
        println!("== Cosmos Computer Use / AT-SPI probe ==");
        println!("display-server: {display}");
        println!(
            "capability verdict:\n  window_capture: {}\n  application_text (AT-SPI): Ready (best-effort; GTK apps often need GTK_A11Y=1)\n  background_input: {}\n  note: Appshots mark Wayland application_text Unavailable because portal captures cannot uniquely match a window to AT-SPI. Cosmos Computer (Cua) still enumerates AT-SPI when the registry is running.",
            if wayland() {
                "portal ScreenCast (user grant)"
            } else {
                "Ready (native X11)"
            },
            if wayland() {
                "often Unavailable (`background_unavailable`)"
            } else {
                "generally Ready"
            },
        );

        futures::executor::block_on(run(want_title))
    }

    async fn run(want_title: Option<String>) -> anyhow::Result<()> {
        let t0 = Instant::now();
        let connection = AccessibilityConnection::new()
            .await
            .context("AT-SPI connect (is at-spi2-registryd running?)")?;
        println!("connect: {} ms", t0.elapsed().as_millis());

        let t1 = Instant::now();
        let registry = connection.root_accessible_on_registry().await?;
        let bus = atspi::zbus::fdo::DBusProxy::new(connection.connection()).await?;
        let apps = registry.get_children().await?;
        println!(
            "observe(apps): {} applications on the bus in {} ms",
            apps.len(),
            t1.elapsed().as_millis()
        );

        let mut windows = Vec::new();
        for app_ref in &apps {
            let Some(name) = app_ref.name() else { continue };
            let pid = bus
                .get_connection_unix_process_id(name.clone().into())
                .await
                .unwrap_or(0);
            let app = match connection.object_as_accessible(app_ref).await {
                Ok(a) => a,
                Err(_) => continue,
            };
            let app_name = app.name().await.unwrap_or_default();
            let children = app.get_children().await.unwrap_or_default();
            for win_ref in children {
                let win = match connection.object_as_accessible(&win_ref).await {
                    Ok(w) => w,
                    Err(_) => continue,
                };
                let role = win.get_role().await.unwrap_or(Role::Invalid);
                let title = win.name().await.unwrap_or_default();
                let states = win.get_state().await.unwrap_or_default();
                let active = states.contains(State::Active) || states.contains(State::Focused);
                windows.push((app_name.clone(), pid, role, title, active, win_ref));
            }
        }
        println!("discover(windows): {} top-level windows", windows.len());
        for (app, pid, role, title, active, _) in &windows {
            println!(
                "  [{}] {} (pid {}) — {}{}",
                role.name(),
                if title.is_empty() {
                    "<untitled>"
                } else {
                    title
                },
                pid,
                app,
                if *active { " · ACTIVE" } else { "" }
            );
        }

        let target = want_title
            .as_ref()
            .and_then(|t| windows.iter().find(|w| &w.3 == t))
            .or_else(|| windows.iter().find(|w| w.4 && !w.3.is_empty()))
            .or_else(|| windows.iter().find(|w| !w.3.is_empty()));

        if let Some((app, _, _, title, _, win_ref)) = target {
            println!("\ntraverse target: \"{title}\" ({app})");
            let t2 = Instant::now();
            let (lines, nodes) = traverse(&connection, win_ref.clone()).await;
            println!(
                "observe(semantic tree): {nodes} nodes in {} ms",
                t2.elapsed().as_millis()
            );
            for line in lines.lines().take(40) {
                println!("  {line}");
            }
        } else {
            println!("\n(no titled window available to traverse in this session)");
        }

        anyhow::ensure!(!apps.is_empty(), "no applications on the AT-SPI bus");
        println!(
            "\nRESULT: AT-SPI reachable on {}.",
            if wayland() { "Wayland" } else { "X11" }
        );
        Ok(())
    }

    async fn traverse(
        connection: &AccessibilityConnection,
        root: atspi::ObjectRefOwned,
    ) -> (String, usize) {
        let started = Instant::now();
        let mut queue = VecDeque::from([(root, 0usize)]);
        let mut out = String::new();
        let mut nodes = 0usize;
        while let Some((obj, depth)) = queue.pop_front() {
            if depth > MAX_DEPTH || nodes >= MAX_NODES || started.elapsed() >= DEADLINE {
                break;
            }
            nodes += 1;
            let Ok(acc) = connection.object_as_accessible(&obj).await else {
                continue;
            };
            let role = acc.get_role().await.unwrap_or(Role::Invalid);
            let name = acc.name().await.unwrap_or_default();
            let interfaces = acc.get_interfaces().await.ok();
            let text = if role != Role::PasswordText
                && interfaces.is_some_and(|s| s.contains(Interface::Text))
            {
                read_text(&acc).await.unwrap_or_default()
            } else {
                String::new()
            };
            let mut fields = Vec::new();
            if !name.trim().is_empty() {
                fields.push(name.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            if !text.trim().is_empty() && text.trim() != name.trim() {
                fields.push(text.split_whitespace().collect::<Vec<_>>().join(" "));
            }
            if !fields.is_empty() {
                out.push_str(&format!(
                    "{}{}: {}\n",
                    "  ".repeat(depth),
                    role.name(),
                    fields.join(" | ")
                ));
            }
            if let Ok(children) = acc.get_children().await {
                queue.extend(children.into_iter().map(|c| (c, depth + 1)));
            }
        }
        (out, nodes)
    }

    async fn read_text(
        acc: &atspi::proxy::accessible::AccessibleProxy<'_>,
    ) -> anyhow::Result<String> {
        let text = TextProxy::builder(acc.inner().connection())
            .destination(acc.inner().destination().clone())?
            .path(acc.inner().path().clone())?
            .build()
            .await?;
        let count = text.character_count().await?.clamp(0, 2048);
        Ok(text.get_text(0, count).await?)
    }
}
