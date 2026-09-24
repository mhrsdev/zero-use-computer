#!/usr/bin/env python3
"""A tiny GTK3 app used by the live Linux backend test.

It exposes, over AT-SPI: a push button, a text entry, a check box, and a
label that reflects their state so the test can verify that accessibility
actions actually took effect. A second page ("Next page" / "Back") and a
modal dialog ("Open dialog" / "OK") exercise screen recognition.
"""
import gi

gi.require_version("Gtk", "3.0")
from gi.repository import GLib, Gtk  # noqa: E402

GLib.set_prgname("cutest")


class App(Gtk.Window):
    def __init__(self):
        super().__init__(title="CU Test")
        self.set_default_size(420, 320)
        self.stack = Gtk.Stack()
        self.add(self.stack)
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        box.set_border_width(12)
        self.stack.add_named(box, "main")

        self.status = Gtk.Label(label="status: idle")
        self.status.set_xalign(0.0)
        box.pack_start(self.status, False, False, 0)

        self.button = Gtk.Button(label="Click Me")
        self.button.connect("clicked", self.on_click)
        box.pack_start(self.button, False, False, 0)

        self.entry = Gtk.Entry()
        self.entry.set_placeholder_text("type here")
        self.entry.connect("changed", self.on_change)
        box.pack_start(self.entry, False, False, 0)

        self.check = Gtk.CheckButton(label="Enable feature")
        self.check.connect("toggled", self.on_toggle)
        box.pack_start(self.check, False, False, 0)

        nav = Gtk.Box(spacing=8)
        next_btn = Gtk.Button(label="Next page")
        next_btn.connect("clicked", lambda _b: self.stack.set_visible_child_name("second"))
        nav.pack_start(next_btn, False, False, 0)
        dialog_btn = Gtk.Button(label="Open dialog")
        dialog_btn.connect("clicked", self.on_dialog)
        nav.pack_start(dialog_btn, False, False, 0)
        box.pack_start(nav, False, False, 0)

        page2 = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        page2.set_border_width(12)
        for i in range(5):
            page2.pack_start(Gtk.Label(label="Detail line %d" % i), False, False, 0)
        back = Gtk.Button(label="Back")
        back.connect("clicked", lambda _b: self.stack.set_visible_child_name("main"))
        page2.pack_start(back, False, False, 0)
        self.stack.add_named(page2, "second")

        self.connect("destroy", Gtk.main_quit)

    def on_dialog(self, _btn):
        dlg = Gtk.Dialog(title="CU Dialog", transient_for=self, modal=True)
        dlg.get_content_area().pack_start(Gtk.Label(label="Proceed with the thing?"), False, False, 8)
        dlg.add_button("OK", Gtk.ResponseType.OK)
        dlg.connect("response", lambda d, _r: d.destroy())
        dlg.show_all()

    def on_click(self, _btn):
        self.status.set_text("status: clicked")

    def on_change(self, entry):
        self.status.set_text("entry: " + entry.get_text())

    def on_toggle(self, chk):
        self.status.set_text("feature: " + ("on" if chk.get_active() else "off"))


if __name__ == "__main__":
    win = App()
    win.show_all()
    Gtk.main()
