//! System tray, best effort, behind one cross-platform surface (architecture rule 10).
//!
//! * Windows: the notification area, through the `tray-icon` crate. Its window procedure runs on
//!   the UI thread inside the event loop eframe already pumps.
//! * Linux: the StatusNotifierItem protocol (KDE, GNOME with the AppIndicator extension, Xfce,
//!   Cinnamon, ...) spoken directly over the session bus with `zbus`, plus a minimal
//!   `com.canonical.dbusmenu` menu. That adds no system package; a desktop without a
//!   StatusNotifierWatcher has no tray, and that is reported as unavailable.
//! * Anywhere else: unavailable.
//!
//! The window is hidden to the tray only while an icon is up, so it can never be hidden without
//! a way back. [`TrayController`] owns that rule; everything above the `platform` modules is
//! plain logic with tests.

use crossbeam_channel::Receiver;

/// What the tray reports to the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// The icon is up and clickable.
    Ready,
    /// The icon could not be created, or the tray host went away. The text says why.
    Failed(String),
    /// The user chose "Show Animatic" or activated the icon.
    Show,
    /// The user chose "Run command queue".
    RunQueue,
    /// The user chose "Quit".
    Quit,
}

/// Whether this system has a tray to put an icon in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// The check has not finished; the options stay usable meanwhile.
    Checking,
    Available,
    /// No tray; the text is shown next to the disabled options.
    Unavailable(String),
}

/// Whether the tray icon should exist: wanted by the settings and supported by the system.
pub fn icon_wanted(show_tray_icon: bool, availability: &Availability) -> bool {
    show_tray_icon && *availability == Availability::Available
}

/// Whether a minimized window should be hidden to the tray right now.
///
/// Only a minimize hides, and only while the icon is actually up (`icon_running`): a failed or
/// missing icon leaves the window as a normal minimized window. `suppressed` holds the window
/// visible after a restore until the window system reports it unminimized again.
pub fn should_hide(
    minimize_to_tray: bool,
    icon_running: bool,
    minimized: bool,
    hidden: bool,
    suppressed: bool,
) -> bool {
    minimize_to_tray && icon_running && minimized && !hidden && !suppressed
}

/// An item of the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Item(MenuAction),
    Separator,
}

/// What a menu item does (a `Copy` subset of [`TrayEvent`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Show,
    RunQueue,
    Quit,
}

impl MenuAction {
    fn event(self) -> TrayEvent {
        match self {
            MenuAction::Show => TrayEvent::Show,
            MenuAction::RunQueue => TrayEvent::RunQueue,
            MenuAction::Quit => TrayEvent::Quit,
        }
    }
}

/// One row of the tray menu; `id` is stable and shared by both backends.
#[derive(Debug, Clone, Copy)]
pub struct MenuEntry {
    pub id: i32,
    pub label: &'static str,
    pub kind: MenuKind,
}

/// The tray menu, top to bottom. Only "Run command queue" is ever disabled (see
/// [`Tray::set_queue_enabled`]).
pub const MENU: [MenuEntry; 4] = [
    MenuEntry {
        id: 1,
        label: "Show Animatic",
        kind: MenuKind::Item(MenuAction::Show),
    },
    MenuEntry {
        id: 2,
        label: "Run command queue",
        kind: MenuKind::Item(MenuAction::RunQueue),
    },
    MenuEntry {
        id: 3,
        label: "",
        kind: MenuKind::Separator,
    },
    MenuEntry {
        id: 4,
        label: "Quit",
        kind: MenuKind::Item(MenuAction::Quit),
    },
];

/// The event a menu item raises, if the id names an item.
pub fn menu_event(id: i32) -> Option<TrayEvent> {
    MENU.iter().find(|e| e.id == id).and_then(|e| match e.kind {
        MenuKind::Item(event) => Some(event.event()),
        MenuKind::Separator => None,
    })
}

/// Whether this is the entry the application's queue gating controls.
fn is_queue_entry(entry: &MenuEntry) -> bool {
    matches!(entry.kind, MenuKind::Item(MenuAction::RunQueue))
}

/// Size of the generated tray icon, in pixels.
pub const ICON_SIZE: u32 = 32;

/// The tray icon as RGBA bytes: a blue rounded square with a white play triangle. Generated, so
/// the repository carries no binary asset.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let radius = s * 0.22;
    let half = s / 2.0;
    // Triangle corners in pixels, wound so that the inside is on the left of every edge.
    let tri = [
        (s * 0.38, s * 0.26),
        (s * 0.38, s * 0.74),
        (s * 0.76, s * 0.50),
    ];
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Rounded-square signed distance: negative inside.
            let (qx, qy) = (
                (px - half).abs() - (half - radius),
                (py - half).abs() - (half - radius),
            );
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let sd = outside + qx.max(qy).min(0.0) - radius;
            let square = (0.5 - sd).clamp(0.0, 1.0);
            // Smallest signed distance to the triangle's edges: positive inside.
            let mut inside = f32::MAX;
            for i in 0..3 {
                let (ax, ay) = tri[i];
                let (bx, by) = tri[(i + 1) % 3];
                let (ex, ey) = (bx - ax, by - ay);
                let len = (ex * ex + ey * ey).sqrt();
                // The interior is to the right of each edge on screen (y points down).
                inside = inside.min((ey * (px - ax) - ex * (py - ay)) / len);
            }
            let glyph = (inside + 0.5).clamp(0.0, 1.0);
            let mix = |blue: f32| (blue * (1.0 - glyph) + 255.0 * glyph).round() as u8;
            rgba.extend_from_slice(&[
                mix(52.0),
                mix(120.0),
                mix(246.0),
                (square * 255.0).round() as u8,
            ]);
        }
    }
    rgba
}

/// Handle to a running (or starting) tray icon. Dropping it removes the icon.
pub struct Tray {
    events: Receiver<TrayEvent>,
    backend: platform::Backend,
    queue_enabled: bool,
}

impl Tray {
    /// Puts the icon up. Success or failure arrives as [`TrayEvent::Ready`] or
    /// [`TrayEvent::Failed`] from [`Tray::poll`]; on Linux the bus work runs on its own thread.
    pub fn start(ctx: &egui::Context) -> Tray {
        let (tx, events) = crossbeam_channel::unbounded();
        let backend = platform::start(ctx.clone(), tx);
        Tray {
            events,
            backend,
            queue_enabled: true,
        }
    }

    /// Everything the tray reported since the last call.
    pub fn poll(&self) -> Vec<TrayEvent> {
        self.events.try_iter().collect()
    }

    /// Enables or disables "Run command queue" to follow the application's gating.
    pub fn set_queue_enabled(&mut self, enabled: bool) {
        if self.queue_enabled != enabled {
            self.queue_enabled = enabled;
            self.backend.set_queue_enabled(enabled);
        }
    }
}

/// Owns the tray icon and the hidden-to-tray state of the main window.
pub struct TrayController {
    availability: Availability,
    probe: Option<Receiver<Availability>>,
    tray: Option<Tray>,
    running: bool,
    hidden: bool,
    /// After a restore, a stale "minimized" report must not hide the window again.
    suppressed: bool,
}

/// A request from the tray for the application to carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayRequest {
    /// "Run command queue"; the application applies its own gating before submitting.
    RunQueue,
}

impl TrayController {
    /// Starts the availability check on a background thread (a bus connection on Linux).
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("tray-probe".into())
            .spawn(move || {
                let availability = match platform::probe() {
                    Ok(()) => Availability::Available,
                    Err(reason) => Availability::Unavailable(reason),
                };
                let _ = tx.send(availability);
                repaint.request_repaint();
            });
        let (availability, probe) = match spawned {
            Ok(_) => (Availability::Checking, Some(rx)),
            Err(e) => (
                Availability::Unavailable(format!("the tray check could not start: {e}")),
                None,
            ),
        };
        TrayController {
            availability,
            probe,
            tray: None,
            running: false,
            hidden: false,
            suppressed: false,
        }
    }

    pub fn availability(&self) -> &Availability {
        &self.availability
    }

    /// Runs once per frame: follows the settings, handles tray events and hides a minimized
    /// window. `queue_allowed` is the application's gating for running the command queue.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        show_tray_icon: bool,
        minimize_to_tray: bool,
        queue_allowed: bool,
    ) -> Vec<TrayRequest> {
        let mut requests = Vec::new();
        if let Some(rx) = &self.probe
            && let Ok(availability) = rx.try_recv()
        {
            self.availability = availability;
            self.probe = None;
        }
        let wanted = icon_wanted(show_tray_icon, &self.availability);
        if wanted && self.tray.is_none() {
            self.tray = Some(Tray::start(ctx));
        } else if !wanted && self.tray.is_some() {
            self.drop_tray(ctx);
        }
        let events = self.tray.as_ref().map(Tray::poll).unwrap_or_default();
        for event in events {
            match event {
                TrayEvent::Ready => self.running = true,
                TrayEvent::Failed(reason) => {
                    log::warn!("System tray unavailable: {reason}");
                    self.availability = Availability::Unavailable(reason);
                    self.drop_tray(ctx);
                }
                TrayEvent::Show => self.show_window(ctx),
                TrayEvent::RunQueue => requests.push(TrayRequest::RunQueue),
                TrayEvent::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        if let Some(tray) = &mut self.tray {
            tray.set_queue_enabled(queue_allowed);
        }
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        if !minimized {
            self.suppressed = false;
        }
        if should_hide(
            minimize_to_tray,
            self.running,
            minimized,
            self.hidden,
            self.suppressed,
        ) {
            self.hidden = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        requests
    }

    /// Brings the window back from the tray (or from a plain minimize) and focuses it.
    pub fn show_window(&mut self, ctx: &egui::Context) {
        self.hidden = false;
        self.suppressed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Removes the icon. A window hidden to the tray is shown first: no icon, no way back.
    fn drop_tray(&mut self, ctx: &egui::Context) {
        if self.hidden {
            self.show_window(ctx);
        }
        self.tray = None;
        self.running = false;
    }
}

#[cfg(windows)]
mod platform {
    use std::cell::Cell;

    use crossbeam_channel::Sender;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::{ICON_SIZE, MENU, MenuKind, TrayEvent, icon_rgba, is_queue_entry, menu_event};

    pub fn probe() -> Result<(), String> {
        // The notification area belongs to the shell; `tray-icon` re-adds the icon when the
        // shell restarts.
        Ok(())
    }

    pub struct Backend {
        _icon: Option<TrayIcon>,
        queue: Option<MenuItem>,
        enabled: Cell<bool>,
    }

    pub fn start(ctx: egui::Context, tx: Sender<TrayEvent>) -> Backend {
        match build(ctx, tx.clone()) {
            Ok((icon, queue)) => {
                let _ = tx.send(TrayEvent::Ready);
                Backend {
                    _icon: Some(icon),
                    queue: Some(queue),
                    enabled: Cell::new(true),
                }
            }
            Err(reason) => {
                let _ = tx.send(TrayEvent::Failed(reason));
                Backend {
                    _icon: None,
                    queue: None,
                    enabled: Cell::new(true),
                }
            }
        }
    }

    fn build(ctx: egui::Context, tx: Sender<TrayEvent>) -> Result<(TrayIcon, MenuItem), String> {
        let menu = Menu::new();
        let mut queue = None;
        for entry in MENU {
            match entry.kind {
                MenuKind::Separator => menu
                    .append(&PredefinedMenuItem::separator())
                    .map_err(|e| e.to_string())?,
                MenuKind::Item(_) => {
                    let item = MenuItem::with_id(entry.id.to_string(), entry.label, true, None);
                    menu.append(&item).map_err(|e| e.to_string())?;
                    if is_queue_entry(&entry) {
                        queue = Some(item);
                    }
                }
            }
        }
        let queue = queue.ok_or("the tray menu has no queue entry")?;
        let icon = Icon::from_rgba(icon_rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE)
            .map_err(|e| e.to_string())?;

        // Both callbacks run on the UI thread, inside the event loop eframe pumps; they only
        // queue the event and ask for a frame.
        let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(event) = event.id.0.parse().ok().and_then(menu_event) {
                let _ = menu_tx.send(event);
                menu_ctx.request_repaint();
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let show = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if show {
                let _ = tx.send(TrayEvent::Show);
                ctx.request_repaint();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Animatic")
            .with_icon(icon)
            .build()
            .map_err(|e| format!("the notification area refused the icon: {e}"))?;
        Ok((icon, queue))
    }

    impl Backend {
        pub fn set_queue_enabled(&self, enabled: bool) {
            if let Some(item) = &self.queue
                && self.enabled.replace(enabled) != enabled
            {
                item.set_enabled(enabled);
            }
        }
    }

    impl Drop for Backend {
        fn drop(&mut self) {
            MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
            TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
    use zbus::blocking::Connection;
    use zbus::blocking::connection::Builder;
    use zbus::blocking::fdo::DBusProxy;
    use zbus::names::BusName;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
    use zbus::{fdo, interface};

    use super::{
        ICON_SIZE, MENU, MenuEntry, MenuKind, TrayEvent, icon_rgba, is_queue_entry, menu_event,
    };

    const WATCHER: &str = "org.kde.StatusNotifierWatcher";
    const WATCHER_PATH: &str = "/StatusNotifierWatcher";
    const ITEM_PATH: &str = "/StatusNotifierItem";
    const MENU_PATH: &str = "/MenuBar";
    const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
    /// How often the thread checks that the tray host is still there.
    const HOST_CHECK: Duration = Duration::from_secs(5);

    type Pixmap = Vec<(i32, i32, Vec<u8>)>;
    type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

    /// The session bus, or in tests the private bus named by `TEST_BUS`.
    fn bus() -> zbus::Result<Builder<'static>> {
        #[cfg(test)]
        if let Some(address) = TEST_BUS.lock().expect("bus address lock").clone() {
            return Builder::address(address.parse::<zbus::Address>()?);
        }
        Builder::session()
    }

    #[cfg(test)]
    pub static TEST_BUS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    pub fn probe() -> Result<(), String> {
        let connection = bus()
            .and_then(Builder::build)
            .map_err(|e| format!("there is no D-Bus session bus to reach a tray host: {e}"))?;
        host_present(&connection)
    }

    fn host_present(connection: &Connection) -> Result<(), String> {
        let bus = DBusProxy::new(connection).map_err(|e| e.to_string())?;
        let name = BusName::try_from(WATCHER).map_err(|e| e.to_string())?;
        match bus.name_has_owner(name) {
            Ok(true) => Ok(()),
            Ok(false) => Err("no system tray host is running (nothing owns \
                              org.kde.StatusNotifierWatcher; GNOME needs the AppIndicator extension)"
                .into()),
            Err(e) => Err(format!("the session bus did not answer: {e}")),
        }
    }

    enum Command {
        QueueEnabled(bool),
    }

    pub struct Backend {
        commands: Sender<Command>,
    }

    pub fn start(ctx: egui::Context, tx: Sender<TrayEvent>) -> Backend {
        let (commands, receiver) = crossbeam_channel::unbounded();
        let failed = tx.clone();
        let failed_ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                if let Err(reason) = run(ctx, tx.clone(), receiver) {
                    let _ = tx.send(TrayEvent::Failed(reason));
                }
            });
        if let Err(e) = spawned {
            let _ = failed.send(TrayEvent::Failed(format!(
                "the tray thread could not start: {e}"
            )));
            failed_ctx.request_repaint();
        }
        Backend { commands }
    }

    impl Backend {
        pub fn set_queue_enabled(&self, enabled: bool) {
            let _ = self.commands.send(Command::QueueEnabled(enabled));
        }
    }

    /// Owns the bus connection until the backend is dropped (the command channel closes).
    fn run(
        ctx: egui::Context,
        tx: Sender<TrayEvent>,
        commands: Receiver<Command>,
    ) -> Result<(), String> {
        let shared = Arc::new(Shared {
            queue_enabled: AtomicBool::new(true),
            revision: AtomicU32::new(1),
        });
        let notify = Notifier {
            tx: tx.clone(),
            ctx: ctx.clone(),
        };
        let item = Item {
            notify: notify.clone(),
        };
        let menu = DbusMenu {
            notify: notify.clone(),
            shared: shared.clone(),
        };
        let service = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let connection = bus()
            .and_then(|b| b.name(service.as_str()))
            .and_then(|b| b.serve_at(ITEM_PATH, item))
            .and_then(|b| b.serve_at(MENU_PATH, menu))
            .and_then(Builder::build)
            .map_err(|e| format!("could not publish the tray item on the session bus: {e}"))?;
        host_present(&connection)?;
        connection
            .call_method(
                Some(WATCHER),
                WATCHER_PATH,
                Some("org.kde.StatusNotifierWatcher"),
                "RegisterStatusNotifierItem",
                &service.as_str(),
            )
            .map_err(|e| format!("the tray host refused the item: {e}"))?;
        let _ = tx.send(TrayEvent::Ready);
        ctx.request_repaint();
        loop {
            match commands.recv_timeout(HOST_CHECK) {
                Ok(Command::QueueEnabled(enabled)) => {
                    shared.queue_enabled.store(enabled, Ordering::SeqCst);
                    let revision = shared.revision.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = connection.emit_signal(
                        None::<&str>,
                        MENU_PATH,
                        MENU_INTERFACE,
                        "LayoutUpdated",
                        &(revision, 0i32),
                    );
                }
                Err(RecvTimeoutError::Timeout) => {
                    // A restored window needs a tray to come back from: if the host is gone,
                    // say so and let the application take the window back.
                    host_present(&connection)?;
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    struct Shared {
        queue_enabled: AtomicBool,
        revision: AtomicU32,
    }

    #[derive(Clone)]
    struct Notifier {
        tx: Sender<TrayEvent>,
        ctx: egui::Context,
    }

    impl Notifier {
        fn send(&self, event: TrayEvent) {
            let _ = self.tx.send(event);
            self.ctx.request_repaint();
        }
    }

    /// RGBA to the ARGB32 byte order (network order) StatusNotifierItem pixmaps use.
    fn pixmap() -> Pixmap {
        let rgba = icon_rgba(ICON_SIZE);
        let argb = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        vec![(ICON_SIZE as i32, ICON_SIZE as i32, argb)]
    }

    struct Item {
        notify: Notifier,
    }

    #[interface(name = "org.kde.StatusNotifierItem")]
    impl Item {
        fn activate(&self, _x: i32, _y: i32) {
            self.notify.send(TrayEvent::Show);
        }

        fn secondary_activate(&self, _x: i32, _y: i32) {
            self.notify.send(TrayEvent::Show);
        }

        /// The host shows the `Menu` object itself; nothing to do for a request to.
        fn context_menu(&self, _x: i32, _y: i32) {}

        fn scroll(&self, _delta: i32, _orientation: String) {}

        #[zbus(property)]
        fn category(&self) -> String {
            "ApplicationStatus".into()
        }

        #[zbus(property)]
        fn id(&self) -> String {
            "animatic".into()
        }

        #[zbus(property)]
        fn title(&self) -> String {
            "Animatic".into()
        }

        #[zbus(property)]
        fn status(&self) -> String {
            "Active".into()
        }

        #[zbus(property)]
        fn window_id(&self) -> u32 {
            0
        }

        #[zbus(property)]
        fn icon_name(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn icon_pixmap(&self) -> Pixmap {
            pixmap()
        }

        #[zbus(property)]
        fn overlay_icon_name(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn overlay_icon_pixmap(&self) -> Pixmap {
            Vec::new()
        }

        #[zbus(property)]
        fn attention_icon_name(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn attention_icon_pixmap(&self) -> Pixmap {
            Vec::new()
        }

        #[zbus(property)]
        fn attention_movie_name(&self) -> String {
            String::new()
        }

        #[zbus(property)]
        fn tool_tip(&self) -> (String, Pixmap, String, String) {
            (String::new(), Vec::new(), "Animatic".into(), String::new())
        }

        #[zbus(property)]
        fn item_is_menu(&self) -> bool {
            false
        }

        #[zbus(property)]
        fn menu(&self) -> OwnedObjectPath {
            OwnedObjectPath::try_from(MENU_PATH).expect("a valid object path")
        }
    }

    struct DbusMenu {
        notify: Notifier,
        shared: Arc<Shared>,
    }

    impl DbusMenu {
        fn properties(&self, entry: &MenuEntry) -> HashMap<String, OwnedValue> {
            let mut props = HashMap::new();
            match entry.kind {
                MenuKind::Separator => {
                    props.insert("type".to_string(), owned("separator"));
                }
                MenuKind::Item(_) => {
                    props.insert("label".to_string(), owned(entry.label));
                    let enabled =
                        !is_queue_entry(entry) || self.shared.queue_enabled.load(Ordering::SeqCst);
                    props.insert("enabled".to_string(), owned(enabled));
                }
            }
            props
        }

        fn leaf(&self, entry: &MenuEntry) -> Node {
            (entry.id, self.properties(entry), Vec::new())
        }
    }

    fn owned<'a, T: Into<Value<'a>>>(value: T) -> OwnedValue {
        OwnedValue::try_from(value.into()).expect("a value without file descriptors")
    }

    #[interface(name = "com.canonical.dbusmenu")]
    impl DbusMenu {
        fn get_layout(
            &self,
            parent_id: i32,
            _recursion_depth: i32,
            _property_names: Vec<String>,
        ) -> fdo::Result<(u32, Node)> {
            let revision = self.shared.revision.load(Ordering::SeqCst);
            if parent_id == 0 {
                let children = MENU.iter().map(|entry| owned(self.leaf(entry))).collect();
                let mut props = HashMap::new();
                props.insert("children-display".to_string(), owned("submenu"));
                return Ok((revision, (0, props, children)));
            }
            MENU.iter()
                .find(|entry| entry.id == parent_id)
                .map(|entry| (revision, self.leaf(entry)))
                .ok_or_else(|| fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))
        }

        fn get_group_properties(
            &self,
            ids: Vec<i32>,
            _property_names: Vec<String>,
        ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
            let wanted = |id: i32| ids.is_empty() || ids.contains(&id);
            MENU.iter()
                .filter(|entry| wanted(entry.id))
                .map(|entry| (entry.id, self.properties(entry)))
                .collect()
        }

        fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedValue> {
            MENU.iter()
                .find(|entry| entry.id == id)
                .and_then(|entry| self.properties(entry).remove(&name))
                .ok_or_else(|| fdo::Error::InvalidArgs(format!("no property {name} on item {id}")))
        }

        fn event(&self, id: i32, event_id: String, _data: OwnedValue, _timestamp: u32) {
            if event_id == "clicked"
                && let Some(event) = menu_event(id)
            {
                self.notify.send(event);
            }
        }

        fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
            for (id, event_id, data, timestamp) in events {
                self.event(id, event_id, data, timestamp);
            }
            Vec::new()
        }

        /// `true` makes the host fetch the layout again, so a menu opened after the queue
        /// gating changed is never stale.
        fn about_to_show(&self, _id: i32) -> bool {
            true
        }

        fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
            (vec![0], Vec::new())
        }

        #[zbus(property)]
        fn version(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn text_direction(&self) -> String {
            "ltr".into()
        }

        #[zbus(property)]
        fn status(&self) -> String {
            "normal".into()
        }

        #[zbus(property)]
        fn icon_theme_path(&self) -> Vec<String> {
            Vec::new()
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use crossbeam_channel::Sender;

    use super::TrayEvent;

    pub fn probe() -> Result<(), String> {
        Err("the system tray is only implemented for Windows and Linux".into())
    }

    pub struct Backend;

    pub fn start(_ctx: egui::Context, tx: Sender<TrayEvent>) -> Backend {
        let _ = tx.send(TrayEvent::Failed(
            "the system tray is only implemented for Windows and Linux".into(),
        ));
        Backend
    }

    impl Backend {
        pub fn set_queue_enabled(&self, _enabled: bool) {}
    }
}

/// Text for the disabled tray options: why they cannot be used here.
pub fn unavailable_reason(availability: &Availability) -> Option<&str> {
    match availability {
        Availability::Unavailable(reason) => Some(reason),
        Availability::Checking | Availability::Available => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_is_wanted_only_when_enabled_and_supported() {
        let ok = Availability::Available;
        let no = Availability::Unavailable("no tray".into());
        assert!(icon_wanted(true, &ok));
        assert!(!icon_wanted(false, &ok));
        assert!(!icon_wanted(true, &no));
        assert!(
            !icon_wanted(true, &Availability::Checking),
            "wait for the check"
        );
    }

    #[test]
    fn only_a_minimize_with_a_running_icon_hides_the_window() {
        // (minimize_to_tray, icon_running, minimized, hidden, suppressed)
        assert!(should_hide(true, true, true, false, false));
        assert!(!should_hide(false, true, true, false, false), "option off");
        assert!(
            !should_hide(true, false, true, false, false),
            "no icon: never hide without a way back"
        );
        assert!(
            !should_hide(true, true, false, false, false),
            "not minimized"
        );
        assert!(
            !should_hide(true, true, true, true, false),
            "already hidden"
        );
        assert!(
            !should_hide(true, true, true, false, true),
            "a stale report right after a restore"
        );
    }

    #[test]
    fn the_menu_offers_show_queue_and_quit() {
        let labels: Vec<_> = MENU
            .iter()
            .filter(|e| matches!(e.kind, MenuKind::Item(_)))
            .map(|e| e.label)
            .collect();
        assert_eq!(labels, ["Show Animatic", "Run command queue", "Quit"]);
        let events: Vec<_> = MENU.iter().filter_map(|e| menu_event(e.id)).collect();
        assert_eq!(
            events,
            [TrayEvent::Show, TrayEvent::RunQueue, TrayEvent::Quit]
        );
        let ids: std::collections::BTreeSet<_> = MENU.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), MENU.len(), "ids are unique");
        assert_eq!(menu_event(0), None, "the root is not an item");
        assert_eq!(MENU.iter().filter(|e| is_queue_entry(e)).count(), 1);
    }

    #[test]
    fn unavailable_systems_explain_themselves() {
        assert_eq!(unavailable_reason(&Availability::Available), None);
        assert_eq!(unavailable_reason(&Availability::Checking), None);
        assert_eq!(
            unavailable_reason(&Availability::Unavailable("why".into())),
            Some("why")
        );
    }

    #[test]
    fn the_generated_icon_is_a_rounded_square_with_a_glyph() {
        let size = ICON_SIZE as usize;
        let rgba = icon_rgba(ICON_SIZE);
        assert_eq!(rgba.len(), size * size * 4);
        let pixel = |x: usize, y: usize| &rgba[(y * size + x) * 4..][..4];
        assert_eq!(pixel(0, 0)[3], 0, "the corner is transparent");
        assert_eq!(pixel(size / 2, 3)[3], 255, "the body is opaque");
        assert_eq!(
            pixel(size / 2, size / 2),
            [255, 255, 255, 255],
            "the glyph is white"
        );
        assert_eq!(
            pixel(size - 5, size / 2 - 1)[..3],
            [52, 120, 246],
            "blue around it"
        );
    }
}

/// The Linux backend against a stand-in tray host on a private session bus: what a desktop's
/// StatusNotifierWatcher and panel would see and do. Skipped where `dbus-daemon` is missing.
#[cfg(all(test, target_os = "linux"))]
mod linux_host_tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use crossbeam_channel::{Receiver, Sender};
    use zbus::blocking::Connection;
    use zbus::blocking::connection::Builder;
    use zbus::interface;
    use zbus::zvariant::{OwnedValue, Value};

    use super::platform::TEST_BUS;
    use super::*;

    /// A private `dbus-daemon`, stopped on drop; the tray backend is pointed at it.
    struct PrivateBus {
        child: Child,
        address: String,
    }

    impl PrivateBus {
        fn start() -> Option<PrivateBus> {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let mut address = String::new();
            BufReader::new(child.stdout.take()?)
                .read_line(&mut address)
                .ok()?;
            let address = address.trim().to_string();
            if address.is_empty() {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            *TEST_BUS.lock().expect("bus address lock") = Some(address.clone());
            Some(PrivateBus { child, address })
        }

        fn connect(&self) -> Connection {
            Builder::address(self.address.as_str())
                .and_then(Builder::build)
                .expect("connect to the private bus")
        }
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            *TEST_BUS.lock().expect("bus address lock") = None;
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// The part of a StatusNotifierWatcher the item talks to.
    struct Watcher {
        registered: Sender<String>,
    }

    #[interface(name = "org.kde.StatusNotifierWatcher")]
    impl Watcher {
        fn register_status_notifier_item(&self, service: String) {
            let _ = self.registered.send(service);
        }
    }

    fn start_host(bus: &PrivateBus) -> (Connection, Receiver<String>) {
        let (registered, receiver) = crossbeam_channel::unbounded();
        let connection = Builder::address(bus.address.as_str())
            .and_then(|b| b.name("org.kde.StatusNotifierWatcher"))
            .and_then(|b| b.serve_at("/StatusNotifierWatcher", Watcher { registered }))
            .and_then(Builder::build)
            .expect("start the stand-in host");
        (connection, receiver)
    }

    /// Drives frames until `done` or fifteen seconds pass; returns the requests seen.
    fn run_frames(
        controller: &mut TrayController,
        ctx: &egui::Context,
        queue_allowed: bool,
        mut done: impl FnMut(&TrayController) -> bool,
    ) -> Vec<TrayRequest> {
        let end = Instant::now() + Duration::from_secs(15);
        let mut requests = Vec::new();
        while Instant::now() < end {
            requests.extend(controller.update(ctx, true, true, queue_allowed));
            if done(controller) {
                return requests;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("timed out; availability {:?}", controller.availability());
    }

    /// Frames for a fixed time, collecting what the tray asked for.
    fn run_for(
        controller: &mut TrayController,
        ctx: &egui::Context,
        queue_allowed: bool,
        time: Duration,
    ) -> Vec<TrayRequest> {
        let end = Instant::now() + time;
        run_frames(controller, ctx, queue_allowed, |_| Instant::now() >= end)
    }

    type Layout = (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>));
    type Item = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

    fn layout_items(client: &Connection, service: &str) -> Vec<Item> {
        let reply = client
            .call_method(
                Some(service),
                "/MenuBar",
                Some("com.canonical.dbusmenu"),
                "GetLayout",
                &(0i32, -1i32, Vec::<String>::new()),
            )
            .expect("GetLayout");
        let (_revision, (_root, _props, children)): Layout =
            reply.body().deserialize().expect("a (u(ia{sv}av)) reply");
        children
            .into_iter()
            .map(|child| {
                let value: Value = child.into();
                value.downcast().expect("every child is a (ia{sv}av)")
            })
            .collect()
    }

    fn is_enabled(item: &Item) -> bool {
        item.1
            .get("enabled")
            .map(|v| bool::try_from(v.try_clone().unwrap()).unwrap())
            .unwrap_or(true)
    }

    fn label(item: &Item) -> Option<String> {
        item.1
            .get("label")
            .map(|v| String::try_from(v.try_clone().unwrap()).unwrap())
    }

    fn click(client: &Connection, service: &str, id: i32, kind: &str) {
        client
            .call_method(
                Some(service),
                "/MenuBar",
                Some("com.canonical.dbusmenu"),
                "Event",
                &(id, kind, Value::from(0i32), 0u32),
            )
            .expect("Event");
    }

    #[test]
    fn a_tray_host_sees_the_item_and_its_clicks_reach_the_application() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipped: dbus-daemon is not available");
            return;
        };
        let ctx = egui::Context::default();

        // No host on the bus: the tray is unavailable, so nothing would ever be hidden.
        let mut without_host = TrayController::new(&ctx);
        run_frames(&mut without_host, &ctx, true, |c| {
            matches!(c.availability(), Availability::Unavailable(_))
        });
        assert!(!without_host.running);

        // With a host: the item registers itself and is announced.
        let (host, registered) = start_host(&bus);
        let mut controller = TrayController::new(&ctx);
        run_frames(&mut controller, &ctx, true, |c| {
            *c.availability() == Availability::Available
        });
        let mut service = None;
        run_frames(&mut controller, &ctx, true, |_| {
            service = service.take().or_else(|| registered.try_recv().ok());
            service.is_some()
        });
        let service = service.expect("the item registered with the host");
        assert!(
            service.starts_with("org.kde.StatusNotifierItem-"),
            "{service}"
        );
        run_frames(&mut controller, &ctx, true, |c| c.running);

        // The panel reads its properties: identity, a menu and a pixmap in ARGB32 order.
        let client = bus.connect();
        let all: HashMap<String, OwnedValue> = client
            .call_method(
                Some(service.as_str()),
                "/StatusNotifierItem",
                Some("org.freedesktop.DBus.Properties"),
                "GetAll",
                &"org.kde.StatusNotifierItem",
            )
            .unwrap()
            .body()
            .deserialize()
            .unwrap();
        for property in ["Category", "Id", "Status", "IconPixmap", "Menu", "ToolTip"] {
            assert!(all.contains_key(property), "{property}");
        }
        let pixmap: Vec<(i32, i32, Vec<u8>)> =
            all["IconPixmap"].try_clone().unwrap().try_into().unwrap();
        assert_eq!(
            (pixmap[0].0, pixmap[0].1),
            (ICON_SIZE as i32, ICON_SIZE as i32)
        );
        assert_eq!(pixmap[0].2.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert_eq!(pixmap[0].2[0], 0, "the corner is transparent (alpha first)");

        // The menu is Show Animatic, Run command queue, Quit.
        let items = layout_items(&client, &service);
        let labels: Vec<_> = items.iter().filter_map(label).collect();
        assert_eq!(labels, ["Show Animatic", "Run command queue", "Quit"]);
        assert!(items.iter().all(is_enabled));

        // Choosing Run command queue reaches the application as a request; other event
        // kinds and unknown ids do nothing.
        click(&client, &service, 2, "clicked");
        let requests = run_for(&mut controller, &ctx, true, Duration::from_millis(500));
        assert_eq!(requests, [TrayRequest::RunQueue]);
        click(&client, &service, 2, "hovered");
        click(&client, &service, 99, "clicked");
        let requests = run_for(&mut controller, &ctx, true, Duration::from_millis(400));
        assert!(requests.is_empty());

        // The item follows the application's gating of the queue, and only that item.
        run_for(&mut controller, &ctx, false, Duration::from_millis(500));
        let enabled: Vec<_> = layout_items(&client, &service)
            .iter()
            .map(is_enabled)
            .collect();
        // Show, Run command queue, separator, Quit.
        assert_eq!(enabled, [true, false, true, true]);
        run_for(&mut controller, &ctx, true, Duration::from_millis(500));
        assert!(layout_items(&client, &service).iter().all(is_enabled));

        // A host that goes away takes the tray down: the icon is dropped (a hidden window is
        // shown again by `drop_tray`) and the tray is reported unavailable.
        drop(host);
        run_frames(&mut controller, &ctx, true, |c| {
            matches!(c.availability(), Availability::Unavailable(_))
        });
        assert!(!controller.running);
    }
}
