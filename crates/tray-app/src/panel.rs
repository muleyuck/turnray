//! The panel the tray icon opens: the agent list `ui::panel_model` works out, which is
//! display-only, or the settings view `ui::settings_model` works out, with the ⚙ (between
//! the two) and Quit buttons under them. Nothing here decides what to show.

use std::ptr::NonNull;

use agent_core::{Status, Style};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSBackingStoreType, NSBox, NSBoxType, NSButton, NSColor, NSEvent, NSEventMask, NSFont, NSImage,
    NSImageView, NSLayoutAttribute, NSLayoutConstraint, NSLayoutConstraintOrientation,
    NSLayoutPriorityDefaultHigh, NSLayoutPriorityDefaultLow, NSLineBreakMode, NSPanel,
    NSPopUpMenuWindowLevel, NSResponder, NSScreen, NSScrollView, NSSegmentSwitchTracking,
    NSSegmentedControl, NSStackView, NSStackViewGravity, NSStatusBarButton, NSStatusItem,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowCollectionBehavior, NSWindowDidMoveNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{
    NSArray, NSData, NSEdgeInsets, NSNotification, NSNotificationCenter, NSObject, NSPoint, NSRect,
    NSSize, NSString,
};
use tao::event_loop::EventLoopProxy;

use crate::backend::UserEvent;
use crate::images;
use crate::ui::{self, AgentCard, Dir, PanelContent, PanelGroup, Rect, SettingsView};

const WIDTH: f64 = 320.0;
const CORNER_RADIUS: f64 = 10.0;
const INSET: f64 = 12.0;
/// How far a card sits in from its group's header.
const CARD_INDENT: f64 = 26.0;
const ICON_SIZE: f64 = 18.0;
/// A priority row's ↑ and ↓ buttons.
const ARROW_WIDTH: f64 = 22.0;
/// The ⚙ button, held while it swaps to the back arrow, which is narrower.
const SETTINGS_BUTTON_WIDTH: f64 = 40.0;
/// The scrolled part never takes more of the screen than this, so the buttons stay on it.
const MAX_SCREEN_SHARE: f64 = 0.6;
const ESCAPE_KEY_CODE: u16 = 53;

struct TargetIvars {
    proxy: EventLoopProxy<UserEvent>,
}

define_class!(
    /// Receives the buttons' clicks and passes them to the event loop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayPanelTarget"]
    #[ivars = TargetIvars]
    struct Target;

    impl Target {
        #[unsafe(method(settings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().proxy.send_event(UserEvent::Settings);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().proxy.send_event(UserEvent::Quit);
        }

        #[unsafe(method(style:))]
        fn style(&self, sender: Option<&NSSegmentedControl>) {
            let Some(sender) = sender else {
                return;
            };
            let style = match sender.selectedSegment() {
                0 => Style::Simple,
                1 => Style::Full,
                _ => return,
            };
            let _ = self.ivars().proxy.send_event(UserEvent::SetStyle(style));
        }

        #[unsafe(method(moveUp:))]
        fn move_up(&self, sender: Option<&NSButton>) {
            self.send_move(sender, Dir::Up);
        }

        #[unsafe(method(moveDown:))]
        fn move_down(&self, sender: Option<&NSButton>) {
            self.send_move(sender, Dir::Down);
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(proxy: EventLoopProxy<UserEvent>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { proxy });
        unsafe { msg_send![super(this), init] }
    }

    /// The button's tag is the status's place in `Status::ALL`. It had the keyboard if it
    /// is the window's first responder: pressed with Space rather than clicked.
    fn send_move(&self, sender: Option<&NSButton>, dir: Dir) {
        let Some(sender) = sender else {
            return;
        };
        let Some(&status) = usize::try_from(sender.tag())
            .ok()
            .and_then(|i| Status::ALL.get(i))
        else {
            return;
        };
        let keyboard = sender
            .window()
            .and_then(|w| w.firstResponder())
            .is_some_and(|r| std::ptr::eq(Retained::as_ptr(&r).cast::<NSButton>(), sender));
        let _ = self.ivars().proxy.send_event(UserEvent::Move {
            status,
            dir,
            keyboard,
        });
    }
}

define_class!(
    /// The scroll view's document view. Flipped, so what it holds hangs from the top: it
    /// opens at its first line and keeps its scroll position when its height changes.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayFlippedView"]
    struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

define_class!(
    /// The panel's window. Borderless, so it has no title bar; non-activating, so it takes
    /// the keyboard without bringing the app forward, and closing it leaves the app that
    /// was in front where it was, as a menu does.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "TurnrayPanelWindow"]
    struct PanelWindow;

    impl PanelWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            // A borderless window refuses by default, which would keep Esc from it.
            true
        }

        /// The panel takes no typing, and a key nothing handles would beep. Only the beep
        /// goes: Tab and Space still move to and press the buttons with Full Keyboard
        /// Access, which a no-op `keyDown:` would swallow too.
        #[unsafe(method(noResponderFor:))]
        fn no_responder_for(&self, selector: Sel) {
            if selector != sel!(keyDown:) {
                unsafe { msg_send![super(self), noResponderFor: selector] }
            }
        }
    }
);

impl PanelWindow {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 100.0));
        let window: Retained<Self> = unsafe {
            msg_send![
                Self::alloc(mtm),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        // Owned through `Retained`; AppKit must not release it on close as well.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setLevel(NSPopUpMenuWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                // Left out of Mission Control and of window cycling, like a menu.
                | NSWindowCollectionBehavior::Transient
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // Closing is ours (see `Panel::watch`), not AppKit's.
        window.setHidesOnDeactivate(false);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window
    }
}

/// The panel's background: the translucent material popovers use, with rounded corners.
fn background(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let effect = NSVisualEffectView::new(mtm);
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setWantsLayer(true);
    // Through the layer by message: typing it would pull in QuartzCore for two setters.
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![&*effect, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: CORNER_RADIUS];
            let _: () = msg_send![&*layer, setMasksToBounds: true];
        }
    }
    effect
}

fn rect(r: NSRect) -> Rect {
    Rect {
        x: r.origin.x,
        y: r.origin.y,
        width: r.size.width,
        height: r.size.height,
    }
}

/// What is registered while the panel is open, to close it or move it with the icon.
struct Watchers {
    monitors: Vec<Retained<AnyObject>>,
    observers: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

/// One priority row of the settings view, rewritten in place by `set_settings`.
struct PriorityRowViews {
    rank: Retained<NSTextField>,
    badge: Retained<NSImageView>,
    name: Retained<NSTextField>,
    note: Retained<NSTextField>,
    up: Retained<NSButton>,
    down: Retained<NSButton>,
}

pub struct Panel {
    mtm: MainThreadMarker,
    proxy: EventLoopProxy<UserEvent>,
    window: Retained<PanelWindow>,
    root: Retained<NSView>,
    document: Retained<FlippedView>,
    list: Retained<NSStackView>,
    settings: Retained<NSStackView>,
    style_control: Retained<NSSegmentedControl>,
    rows: Vec<PriorityRowViews>,
    /// One badge per status, in `Status::ALL` order, decoded once.
    badges: Vec<Retained<NSImage>>,
    /// Holds the scroll view to the shown view's height, up to the screen share.
    scroll_height: Retained<NSLayoutConstraint>,
    /// The document ends at one of these, whichever view is on show.
    list_bottom: Retained<NSLayoutConstraint>,
    settings_bottom: Retained<NSLayoutConstraint>,
    settings_button: Retained<NSButton>,
    /// What the ⚙ button shows: a gear on the list, a way back to it on the settings view.
    gear: Option<Retained<NSImage>>,
    back: Option<Retained<NSImage>>,
    showing_settings: bool,
    /// Kept alive here: a button holds its target weakly.
    _target: Retained<Target>,
    /// The tray button the panel hangs from, while it is open.
    anchor: Option<Retained<NSStatusBarButton>>,
    watchers: Option<Watchers>,
}

fn label(
    text: &str,
    font: &NSFont,
    color: &NSColor,
    mtm: MainThreadMarker,
) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    l.setFont(Some(font));
    l.setTextColor(Some(color));
    l.setUsesSingleLineMode(true);
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    // Below the width the panel gives it, so a long line is cut rather than widening it.
    l.setContentCompressionResistancePriority_forOrientation(
        NSLayoutPriorityDefaultLow,
        NSLayoutConstraintOrientation::Horizontal,
    );
    l
}

/// A template image, so AppKit tints it to the text colour like the menu bar does.
fn template_image(png: &[u8]) -> Retained<NSImage> {
    let image =
        NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(png)).expect("embedded PNG");
    image.setTemplate(true);
    image.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
    image
}

fn icon(png: &[u8], mtm: MainThreadMarker) -> Retained<NSImageView> {
    let view = NSImageView::imageViewWithImage(&template_image(png), mtm);
    view.setContentTintColor(Some(&NSColor::labelColor()));
    activate(&[
        view.widthAnchor().constraintEqualToConstant(ICON_SIZE),
        view.heightAnchor().constraintEqualToConstant(ICON_SIZE),
    ]);
    view
}

fn stack(
    orientation: NSUserInterfaceLayoutOrientation,
    views: &[&NSView],
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_slice(views), mtm);
    s.setOrientation(orientation);
    s
}

fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(constraints));
}

/// Makes `view` as wide as the stack holding it less its insets, so every row shares one
/// width.
fn fill_width(view: &NSView, list: &NSStackView, indent: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate(&[view
        .widthAnchor()
        .constraintEqualToAnchor_constant(&list.widthAnchor(), -(2.0 * INSET + indent))]);
}

fn group_views(group: &PanelGroup, mtm: MainThreadMarker) -> Vec<Retained<NSView>> {
    let name = label(
        group.status.as_str(),
        &NSFont::boldSystemFontOfSize(13.0),
        &NSColor::labelColor(),
        mtm,
    );
    let count = label(
        &group.count.to_string(),
        &NSFont::systemFontOfSize(13.0),
        &NSColor::secondaryLabelColor(),
        mtm,
    );
    let image = icon(images::status_png(group.status), mtm);
    let header = stack(
        NSUserInterfaceLayoutOrientation::Horizontal,
        &[&image, &name, &count],
        mtm,
    );
    header.setSpacing(6.0);
    let mut views: Vec<Retained<NSView>> = vec![Retained::into_super(header)];
    for agent in &group.agents {
        let card = card_view(agent, mtm);
        views.push(Retained::into_super(card));
    }
    views
}

fn card_view(agent: &AgentCard, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let heading = label(
        &agent.heading,
        &NSFont::boldSystemFontOfSize(13.0),
        &NSColor::labelColor(),
        mtm,
    );
    let mut lines: Vec<Retained<NSView>> =
        vec![Retained::into_super(Retained::into_super(heading))];
    if let Some(title) = &agent.title {
        let title = label(
            title,
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            mtm,
        );
        lines.push(Retained::into_super(Retained::into_super(title)));
    }
    let refs: Vec<&NSView> = lines.iter().map(|v| &**v).collect();
    let card = stack(NSUserInterfaceLayoutOrientation::Vertical, &refs, mtm);
    card.setAlignment(NSLayoutAttribute::Leading);
    card.setSpacing(1.0);
    card.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: CARD_INDENT,
        bottom: 0.0,
        right: 0.0,
    });
    card
}

fn message_view(text: &str, warning: bool, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let line = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    line.setAlignment(NSTextAlignment::Center);
    line.setTextColor(Some(&NSColor::secondaryLabelColor()));
    line.setPreferredMaxLayoutWidth(WIDTH - 2.0 * INSET - ICON_SIZE - 8.0);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(8.0);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if warning {
        views.push(Retained::into_super(Retained::into_super(icon(
            images::error_png(),
            mtm,
        ))));
    }
    views.push(Retained::into_super(Retained::into_super(line)));
    let refs: Vec<&NSView> = views.iter().map(|v| &**v).collect();
    row.setViews_inGravity(&NSArray::from_slice(&refs), NSStackViewGravity::Center);
    row.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 0.0,
        bottom: 12.0,
        right: 0.0,
    });
    row
}

fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Separator);
    b
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

/// A button showing an SF Symbol, or `fallback` text where the symbol can't be loaded.
fn symbol_button(
    name: &str,
    description: &str,
    fallback: &str,
    target: &Target,
    action: Sel,
    mtm: MainThreadMarker,
) -> Retained<NSButton> {
    match symbol(name, description) {
        Some(image) => unsafe {
            NSButton::buttonWithImage_target_action(&image, Some(target), Some(action), mtm)
        },
        None => unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(fallback),
                Some(target),
                Some(action),
                mtm,
            )
        },
    }
}

fn section_label(text: &str, mtm: MainThreadMarker) -> Retained<NSTextField> {
    label(
        text,
        &NSFont::boldSystemFontOfSize(11.0),
        &NSColor::secondaryLabelColor(),
        mtm,
    )
}

/// One row of the priority list, empty until `set_settings` fills it in.
fn priority_row(
    target: &Target,
    mtm: MainThreadMarker,
) -> (Retained<NSStackView>, PriorityRowViews) {
    let views = PriorityRowViews {
        rank: label(
            "",
            &NSFont::systemFontOfSize(13.0),
            &NSColor::secondaryLabelColor(),
            mtm,
        ),
        badge: icon(images::status_png(Status::Unknown), mtm),
        name: label(
            "",
            &NSFont::boldSystemFontOfSize(13.0),
            &NSColor::labelColor(),
            mtm,
        ),
        note: label(
            "— not reported by this source",
            &NSFont::systemFontOfSize(11.0),
            &NSColor::secondaryLabelColor(),
            mtm,
        ),
        up: symbol_button("chevron.up", "Move up", "↑", target, sel!(moveUp:), mtm),
        down: symbol_button(
            "chevron.down",
            "Move down",
            "↓",
            target,
            sel!(moveDown:),
            mtm,
        ),
    };
    // The note gives way first, so the rank and the name stay whole.
    for keep in [&views.rank, &views.name] {
        keep.setContentCompressionResistancePriority_forOrientation(
            NSLayoutPriorityDefaultHigh,
            NSLayoutConstraintOrientation::Horizontal,
        );
    }
    // No bezel, so a row is as tall as its badge, like a group header in the list. A bare
    // chevron is only 16×9pt, so each button is held to a badge-high target to click.
    for button in [&views.up, &views.down] {
        button.setBordered(false);
        activate(&[
            button.widthAnchor().constraintEqualToConstant(ARROW_WIDTH),
            button.heightAnchor().constraintEqualToConstant(ICON_SIZE),
        ]);
    }
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(6.0);
    let leading: [&NSView; 4] = [&views.rank, &views.badge, &views.name, &views.note];
    let trailing: [&NSView; 2] = [&views.up, &views.down];
    row.setViews_inGravity(&NSArray::from_slice(&leading), NSStackViewGravity::Leading);
    row.setViews_inGravity(
        &NSArray::from_slice(&trailing),
        NSStackViewGravity::Trailing,
    );
    (row, views)
}

/// The settings view, built once: Style, then the five priority rows.
fn settings_view(
    target: &Target,
    mtm: MainThreadMarker,
) -> (
    Retained<NSStackView>,
    Retained<NSSegmentedControl>,
    Vec<PriorityRowViews>,
) {
    let settings = NSStackView::new(mtm);
    settings.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    settings.setAlignment(NSLayoutAttribute::Leading);
    settings.setSpacing(8.0);
    settings.setEdgeInsets(NSEdgeInsets {
        top: INSET,
        left: INSET,
        bottom: INSET,
        right: INSET,
    });
    settings.setTranslatesAutoresizingMaskIntoConstraints(false);

    let labels = [NSString::from_str("Simple"), NSString::from_str("Full")];
    let style_control = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &NSArray::from_retained_slice(&labels),
            NSSegmentSwitchTracking::SelectOne,
            Some(target),
            Some(sel!(style:)),
            mtm,
        )
    };
    settings.addArrangedSubview(&section_label("Style", mtm));
    settings.addArrangedSubview(&style_control);
    let rule = separator(mtm);
    settings.addArrangedSubview(&rule);
    fill_width(&rule, &settings, 0.0);
    settings.addArrangedSubview(&section_label("Priority", mtm));
    let mut rows = Vec::new();
    for _ in Status::ALL {
        let (row, views) = priority_row(target, mtm);
        settings.addArrangedSubview(&row);
        fill_width(&row, &settings, 0.0);
        rows.push(views);
    }
    (settings, style_control, rows)
}

impl Panel {
    pub fn new(proxy: EventLoopProxy<UserEvent>, mtm: MainThreadMarker) -> Panel {
        let target = Target::new(proxy.clone(), mtm);

        let list = NSStackView::new(mtm);
        list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        list.setAlignment(NSLayoutAttribute::Leading);
        list.setSpacing(6.0);
        list.setEdgeInsets(NSEdgeInsets {
            top: INSET,
            left: INSET,
            bottom: INSET,
            right: INSET,
        });
        list.setTranslatesAutoresizingMaskIntoConstraints(false);
        let (settings, style_control, rows) = settings_view(&target, mtm);
        settings.setHidden(true);

        // The list and the settings view, one shown at a time, both kept in the document
        // so their constraints stay. The document ends where the one on show does.
        let document = FlippedView::new(mtm);
        document.setTranslatesAutoresizingMaskIntoConstraints(false);
        document.addSubview(&list);
        document.addSubview(&settings);
        let list_bottom = list
            .bottomAnchor()
            .constraintEqualToAnchor(&document.bottomAnchor());
        let settings_bottom = settings
            .bottomAnchor()
            .constraintEqualToAnchor(&document.bottomAnchor());

        let scroll = NSScrollView::new(mtm);
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&document));
        let clip = scroll.contentView();

        let settings_button = symbol_button(
            "gearshape",
            "Settings",
            "Settings",
            &target,
            sel!(settings:),
            mtm,
        );
        let quit_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Quit"),
                Some(&target),
                Some(sel!(quit:)),
                mtm,
            )
        };
        let footer = NSStackView::new(mtm);
        footer.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        let settings_view: &NSView = &settings_button;
        let quit_view: &NSView = &quit_button;
        footer.setViews_inGravity(
            &NSArray::from_slice(&[settings_view]),
            NSStackViewGravity::Leading,
        );
        footer.setViews_inGravity(
            &NSArray::from_slice(&[quit_view]),
            NSStackViewGravity::Trailing,
        );
        footer.setEdgeInsets(NSEdgeInsets {
            top: 8.0,
            left: INSET,
            bottom: 8.0,
            right: INSET,
        });
        footer.setTranslatesAutoresizingMaskIntoConstraints(false);

        let rule = separator(mtm);
        rule.setTranslatesAutoresizingMaskIntoConstraints(false);

        let root = NSView::new(mtm);
        root.addSubview(&scroll);
        root.addSubview(&rule);
        root.addSubview(&footer);

        let scroll_height = scroll.heightAnchor().constraintEqualToConstant(0.0);
        activate(&[
            root.widthAnchor().constraintEqualToConstant(WIDTH),
            // Both views hang from the top of the document view, which is as wide as the
            // clip view; `apply_view` picks which one's bottom it ends at.
            list.topAnchor()
                .constraintEqualToAnchor(&document.topAnchor()),
            list.leadingAnchor()
                .constraintEqualToAnchor(&document.leadingAnchor()),
            list.trailingAnchor()
                .constraintEqualToAnchor(&document.trailingAnchor()),
            settings
                .topAnchor()
                .constraintEqualToAnchor(&document.topAnchor()),
            settings
                .leadingAnchor()
                .constraintEqualToAnchor(&document.leadingAnchor()),
            settings
                .trailingAnchor()
                .constraintEqualToAnchor(&document.trailingAnchor()),
            list_bottom.clone(),
            document
                .topAnchor()
                .constraintEqualToAnchor(&clip.topAnchor()),
            document
                .leadingAnchor()
                .constraintEqualToAnchor(&clip.leadingAnchor()),
            document
                .trailingAnchor()
                .constraintEqualToAnchor(&clip.trailingAnchor()),
            scroll
                .topAnchor()
                .constraintEqualToAnchor(&root.topAnchor()),
            scroll
                .leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            scroll_height.clone(),
            settings_button
                .widthAnchor()
                .constraintEqualToConstant(SETTINGS_BUTTON_WIDTH),
            rule.topAnchor()
                .constraintEqualToAnchor(&scroll.bottomAnchor()),
            rule.leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            rule.trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            footer
                .topAnchor()
                .constraintEqualToAnchor(&rule.bottomAnchor()),
            footer
                .leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            footer
                .trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            footer
                .bottomAnchor()
                .constraintEqualToAnchor(&root.bottomAnchor()),
        ]);

        let effect = background(mtm);
        root.setTranslatesAutoresizingMaskIntoConstraints(false);
        effect.addSubview(&root);
        // Top and sides only: the window is sized to the root, not the other way round.
        activate(&[
            root.topAnchor()
                .constraintEqualToAnchor(&effect.topAnchor()),
            root.leadingAnchor()
                .constraintEqualToAnchor(&effect.leadingAnchor()),
            root.trailingAnchor()
                .constraintEqualToAnchor(&effect.trailingAnchor()),
        ]);
        let window = PanelWindow::new(mtm);
        window.setContentView(Some(&effect));

        Panel {
            mtm,
            proxy,
            window,
            root,
            document,
            list,
            settings,
            style_control,
            rows,
            badges: Status::ALL
                .iter()
                .map(|&st| template_image(images::status_png(st)))
                .collect(),
            scroll_height,
            list_bottom,
            settings_bottom,
            settings_button,
            gear: symbol("gearshape", "Settings"),
            back: symbol("chevron.backward", "Back to the list"),
            showing_settings: false,
            _target: target,
            anchor: None,
            watchers: None,
        }
    }

    pub fn is_shown(&self) -> bool {
        self.window.isVisible()
    }

    /// Replaces the list. Done while closed too, so the panel opens on the latest. The
    /// caller refits the panel once it has updated everything (`fit`).
    pub fn set_content(&mut self, content: &PanelContent) {
        for view in self.list.arrangedSubviews().iter() {
            view.removeFromSuperview();
        }
        let mtm = self.mtm;
        match content {
            PanelContent::Empty => {}
            PanelContent::Message { text, warning } => {
                let row = message_view(text, *warning, mtm);
                self.list.addArrangedSubview(&row);
                fill_width(&row, &self.list, 0.0);
            }
            PanelContent::Groups(groups) => {
                for (i, group) in groups.iter().enumerate() {
                    if i > 0 {
                        let rule = separator(mtm);
                        self.list.addArrangedSubview(&rule);
                        fill_width(&rule, &self.list, 0.0);
                    }
                    for view in group_views(group, mtm) {
                        self.list.addArrangedSubview(&view);
                        fill_width(&view, &self.list, 0.0);
                    }
                }
            }
        }
    }

    /// Rewrites the settings view in place, so a control with the keyboard keeps it.
    pub fn set_settings(&mut self, view: &SettingsView) {
        self.style_control.setSelectedSegment(match view.style {
            Style::Simple => 0,
            Style::Full => 1,
        });
        for (views, row) in self.rows.iter().zip(&view.priority) {
            // A status's place in `Status::ALL`: the buttons' tag and its badge.
            let i = Status::ALL
                .iter()
                .position(|&s| s == row.status)
                .expect("ALL holds every status");
            views
                .rank
                .setStringValue(&NSString::from_str(&format!("{}.", row.rank)));
            views.badge.setImage(Some(&self.badges[i]));
            views
                .name
                .setStringValue(&NSString::from_str(row.status.as_str()));
            views.note.setHidden(row.reported);
            views.up.setTag(i as isize);
            views.down.setTag(i as isize);
            views.up.setEnabled(row.can_move_up);
            views.down.setEnabled(row.can_move_down);
        }
    }

    /// After a move made from the keyboard, gives the keyboard to the button that keeps
    /// moving the same status (`ui::focus_after_move`).
    pub fn focus_move_button(&self, row: usize, dir: Dir) {
        if !self.is_shown() || !self.showing_settings {
            return;
        }
        let Some(views) = self.rows.get(row) else {
            return;
        };
        let button: &NSResponder = match dir {
            Dir::Up => &views.up,
            Dir::Down => &views.down,
        };
        self.window.makeFirstResponder(Some(button));
    }

    /// ⚙ switches between the list and the settings view.
    pub fn toggle_settings(&mut self) {
        if !self.is_shown() {
            return;
        }
        self.showing_settings = !self.showing_settings;
        self.apply_view();
        self.document.scrollPoint(NSPoint::new(0.0, 0.0));
        self.fit();
    }

    /// Shows the view `showing_settings` names, and the ⚙ button as what pressing it does:
    /// a gear to open the settings, or a back arrow to the list. The tooltip says which.
    fn apply_view(&self) {
        self.list.setHidden(self.showing_settings);
        self.settings.setHidden(!self.showing_settings);
        // Off before on, so the two never hold the document at once.
        let (off, on) = if self.showing_settings {
            (&self.list_bottom, &self.settings_bottom)
        } else {
            (&self.settings_bottom, &self.list_bottom)
        };
        off.setActive(false);
        on.setActive(true);
        let button = &self.settings_button;
        let (image, fallback, tip) = if self.showing_settings {
            (&self.back, "Back", "Back to the list")
        } else {
            (&self.gear, "Settings", "Settings")
        };
        button.setToolTip(Some(&NSString::from_str(tip)));
        // Text where the symbol can't be loaded, as `symbol_button` does.
        match image {
            Some(image) => {
                button.setTitle(&NSString::from_str(""));
                button.setImage(Some(image));
            }
            None => {
                button.setImage(None);
                button.setTitle(&NSString::from_str(fallback));
            }
        }
    }

    /// The icon's frame and its screen's, read afresh: a status item keeps its right edge,
    /// so its left edge moves whenever the Full style's image changes width.
    fn placement(&self) -> Option<(Rect, Rect)> {
        let bar = self.anchor.as_ref()?.window()?;
        // The screen whose menu bar was clicked, which need not be the main one.
        let screen = bar.screen().or_else(|| NSScreen::mainScreen(self.mtm))?;
        Some((rect(bar.frame()), rect(screen.visibleFrame())))
    }

    /// Fits the scroll view to the view on show, up to the limit, and the window to both,
    /// hanging from the icon. Nothing to fit to until the panel has an icon to hang from.
    pub fn fit(&self) {
        let Some((icon, visible)) = self.placement() else {
            return;
        };
        let shown = if self.showing_settings {
            &self.settings
        } else {
            &self.list
        };
        let content = shown.fittingSize().height;
        self.scroll_height
            .setConstant(content.min(visible.height * MAX_SCREEN_SHARE));
        self.root.layoutSubtreeIfNeeded();
        let f = ui::panel_frame(icon, WIDTH, self.root.fittingSize().height, visible);
        self.window.setFrame_display(
            NSRect::new(NSPoint::new(f.x, f.y), NSSize::new(f.width, f.height)),
            true,
        );
        // The shadow follows the rounded corners only once it is redrawn for the new size.
        self.window.invalidateShadow();
    }

    /// Opens the panel under `item`'s button. Does nothing while the item is hidden.
    pub fn show(&mut self, item: &NSStatusItem) {
        let mtm = self.mtm;
        let Some(button) = item.button(mtm) else {
            return;
        };
        let Some(bar) = button.window() else {
            return;
        };
        self.anchor = Some(button);
        // Always opens on the list.
        self.showing_settings = false;
        self.apply_view();
        self.fit();
        self.window.makeKeyAndOrderFront(None);
        self.keep_highlight();
        self.watch(&bar);
    }

    /// The icon moved or changed width while open: hang from it again.
    pub fn follow_anchor(&self) {
        if self.is_shown() {
            self.fit();
        }
    }

    /// tray-icon unhighlights the button on every mouse up; while open it stays pressed.
    pub fn keep_highlight(&self) {
        if let Some(button) = &self.anchor {
            button.highlight(true);
        }
    }

    pub fn close(&mut self) {
        // Undone even if the window is already gone, so nothing outlives the panel.
        self.unwatch();
        if let Some(button) = self.anchor.take() {
            button.highlight(false);
        }
        self.showing_settings = false;
        self.apply_view();
        if !self.is_shown() {
            return;
        }
        self.window.orderOut(None);
    }

    fn watch(&mut self, bar: &NSWindow) {
        self.unwatch();
        let send_close = {
            let proxy = self.proxy.clone();
            move || {
                let _ = proxy.send_event(UserEvent::ClosePanel);
            }
        };
        let mut monitors = Vec::new();
        let on_click = send_close.clone();
        if let Some(m) = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &RcBlock::new(move |_: NonNull<NSEvent>| on_click()),
        ) {
            monitors.push(m);
        }
        let on_key = send_close.clone();
        // Returning null swallows the Esc; anything else goes on to the panel.
        let on_key_down = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            if unsafe { event.as_ref() }.keyCode() == ESCAPE_KEY_CODE {
                on_key();
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        let local = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown,
                &on_key_down,
            )
        };
        if let Some(m) = local {
            monitors.push(m);
        }
        let center = NSNotificationCenter::defaultCenter();
        // The panel losing the keyboard: Cmd-Tab, or a click in another app.
        let mut observers = vec![unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidResignKeyNotification),
                Some(&self.window),
                None,
                &RcBlock::new(move |_: NonNull<NSNotification>| send_close()),
            )
        }];
        // Sent on, not handled here: the frame is read once the change has settled.
        for name in unsafe { [NSWindowDidResizeNotification, NSWindowDidMoveNotification] } {
            let proxy = self.proxy.clone();
            observers.push(unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    Some(bar),
                    None,
                    &RcBlock::new(move |_: NonNull<NSNotification>| {
                        let _ = proxy.send_event(UserEvent::AnchorMoved);
                    }),
                )
            });
        }
        self.watchers = Some(Watchers {
            monitors,
            observers,
        });
    }

    fn unwatch(&mut self) {
        let Some(w) = self.watchers.take() else {
            return;
        };
        for m in &w.monitors {
            unsafe { NSEvent::removeMonitor(m) };
        }
        let center = NSNotificationCenter::defaultCenter();
        for o in &w.observers {
            let observer: &AnyObject = o.as_ref();
            unsafe { center.removeObserver(observer) };
        }
    }
}
