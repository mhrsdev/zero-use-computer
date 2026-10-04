#!/usr/bin/env python3
"""The apps the agent benchmark (bench/) runs its scenarios against.

One GTK 3 program, a different app per scenario:

  form     "Settings": named fields, radio buttons, a check box
  table    "Inventory": 300 rows, a filter, an Open button
  board    "Board": a full toolbar above a canvas of labelled boxes (the
           labels are painted, invisible to accessibility)
  shapes   "Shapes": the same toolbar above a canvas of shapes, no text
  orders   "Orders": a painted notice, a field, a confirmation dialog
  counter  "Counter": all painted, nothing for accessibility (a game's
           menu, a custom-drawn app): a count and three painted buttons

Everything it shows is fixed (no clock, no randomness), so every run starts
from the same state. What the agent did is written to the JSON file given
with --state on every change, and the benchmark checks it at the end.

  python3 bench_app.py --scenario form --state /tmp/state.json
"""
import argparse
import json
import math
import os

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk  # noqa: E402

TITLES = {
    "form": "Settings",
    "table": "Inventory",
    "board": "Board",
    "shapes": "Shapes",
    "orders": "Orders",
    "counter": "Counter",
}

TOOLBAR = [
    "Select", "Pen", "Pencil", "Brush", "Eraser", "Line", "Arrow",
    "Rectangle", "Ellipse", "Polygon", "Text", "Sticky note", "Connector",
    "Frame", "Image", "Comment", "Undo", "Redo", "Zoom in", "Zoom out",
    "Fit", "Grid", "Layers", "Share",
]


class State:
    """What the agent did, saved after every change."""

    def __init__(self, path):
        self.path = path
        self.data = {}
        self.save()

    def set(self, key, value):
        self.data[key] = value
        self.save()

    def append(self, key, value):
        self.data.setdefault(key, []).append(value)
        self.save()

    def save(self):
        if not self.path:
            return
        tmp = self.path + ".tmp"
        with open(tmp, "w") as f:
            json.dump(self.data, f)
        os.replace(tmp, self.path)


def labelled(grid, row, text, widget):
    label = Gtk.Label(label=text, xalign=0.0)
    label.set_mnemonic_widget(widget)
    # GTK 3 doesn't name a field after its label over AT-SPI; a well-made
    # app names it itself.
    widget.get_accessible().set_name(text)
    grid.attach(label, 0, row, 1, 1)
    grid.attach(widget, 1, row, 1, 1)


def toolbar():
    """Two rows of tool buttons, as drawing apps have."""
    bar = Gtk.Grid(row_spacing=2, column_spacing=2)
    for i, name in enumerate(TOOLBAR):
        bar.attach(Gtk.Button(label=name), i % 12, i // 12, 1, 1)
    return bar


# --------------------------------------------------------------------------
# form


def build_form(win, state):
    state.set("saves", 0)
    root = Gtk.Box(spacing=12)
    side = Gtk.ListBox()
    for name in [
        "General", "Profile", "Notifications", "Privacy", "Security",
        "Appearance", "Language", "Accessibility", "Storage", "Devices",
        "Billing", "About",
    ]:
        side.add(Gtk.Label(label=name, xalign=0.0))
    root.pack_start(side, False, False, 0)

    grid = Gtk.Grid(row_spacing=8, column_spacing=12)
    grid.set_border_width(12)
    name = Gtk.Entry()
    email = Gtk.Entry()
    labelled(grid, 0, "Full name", name)
    labelled(grid, 1, "Email", email)
    countries = Gtk.Box(spacing=6)
    first = None
    radios = {}
    for c in ["Canada", "Germany", "Iran", "Japan", "Brazil"]:
        r = Gtk.RadioButton.new_with_label_from_widget(first, c)
        first = first or r
        radios[c] = r
        countries.pack_start(r, False, False, 0)
    grid.attach(Gtk.Label(label="Country", xalign=0.0), 0, 2, 1, 1)
    grid.attach(countries, 1, 2, 1, 1)
    subscribe = Gtk.CheckButton(label="Subscribe to newsletter")
    grid.attach(subscribe, 1, 3, 1, 1)
    buttons = Gtk.Box(spacing=8)
    cancel = Gtk.Button(label="Cancel")
    save = Gtk.Button(label="Save")
    buttons.pack_end(save, False, False, 0)
    buttons.pack_end(cancel, False, False, 0)
    grid.attach(buttons, 1, 4, 1, 1)

    def on_save(_b):
        country = next(c for c, r in radios.items() if r.get_active())
        state.set("saved", {
            "name": name.get_text(),
            "email": email.get_text(),
            "country": country,
            "subscribe": subscribe.get_active(),
        })
        state.set("saves", state.data["saves"] + 1)

    save.connect("clicked", on_save)
    root.pack_start(grid, True, True, 0)
    win.add(root)
    win.set_default_size(720, 360)


# --------------------------------------------------------------------------
# table

ADJ = ["Red", "Blue", "Quiet", "Rapid", "Solid", "Tiny", "Grand", "Smart",
       "Brisk", "Noble"]
NOUN = ["Widget", "Gadget", "Bracket", "Hinge", "Valve", "Socket", "Pulley",
        "Spring", "Clamp", "Lever", "Bolt", "Gear"]


def build_table(win, state):
    state.set("opened", [])
    store = Gtk.ListStore(str, str, str, int)
    for i in range(300):
        store.append([
            "K-%04d" % i,
            "%s %s" % (ADJ[(i * 7) % len(ADJ)], NOUN[(i * 5) % len(NOUN)]),
            "%.2f" % (5 + ((i * 37) % 500) / 10),
            (i * 7) % 40,
        ])
    query = {"text": ""}
    filtered = store.filter_new()

    def visible(model, it, _data):
        q = query["text"].lower()
        return not q or q in model[it][0].lower() or q in model[it][1].lower()

    filtered.set_visible_func(visible)
    view = Gtk.TreeView(model=filtered)
    for col, title in enumerate(["SKU", "Name", "Price", "Stock"]):
        view.append_column(Gtk.TreeViewColumn(title, Gtk.CellRendererText(), text=col))
    scroll = Gtk.ScrolledWindow()
    scroll.add(view)

    bar = Gtk.Box(spacing=6)
    bar.set_border_width(6)
    search = Gtk.SearchEntry()
    search.set_placeholder_text("Filter by SKU or name")
    search.get_accessible().set_name("Filter")

    def on_search(entry):
        query["text"] = entry.get_text()
        filtered.refilter()

    search.connect("search-changed", on_search)
    bar.pack_start(search, True, True, 0)
    status = Gtk.Label(label="")
    opener = Gtk.Button(label="Open")

    def on_open(_b):
        model, it = view.get_selection().get_selected()
        if it is None:
            status.set_text("Nothing selected")
            return
        sku = model[it][0]
        state.append("opened", sku)
        status.set_text("Opened " + sku)

    opener.connect("clicked", on_open)
    for widget in [opener, Gtk.Button(label="Refresh"), Gtk.Button(label="Export")]:
        bar.pack_start(widget, False, False, 0)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    box.pack_start(bar, False, False, 0)
    box.pack_start(scroll, True, True, 0)
    box.pack_start(status, False, False, 4)
    win.add(box)
    win.set_default_size(900, 600)


# --------------------------------------------------------------------------
# board and shapes: a toolbar above a canvas the tree knows nothing about


def canvas_window(win, state, draw, hit):
    state.set("clicks", [])
    area = Gtk.DrawingArea()
    area.set_size_request(840, 480)
    area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)

    def on_draw(_a, cr):
        cr.set_source_rgb(1, 1, 1)
        cr.paint()
        # Graph-paper lines, as many canvases have.
        cr.set_source_rgb(0.9, 0.9, 0.93)
        cr.set_line_width(1)
        for x in range(0, 841, 20):
            cr.move_to(x + 0.5, 0)
            cr.line_to(x + 0.5, 480)
        for y in range(0, 481, 20):
            cr.move_to(0, y + 0.5)
            cr.line_to(840, y + 0.5)
        cr.stroke()
        draw(cr)

    def on_press(_a, event):
        state.append("clicks", {"hit": hit(event.x, event.y),
                                "x": round(event.x), "y": round(event.y)})

    area.connect("draw", on_draw)
    area.connect("button-press-event", on_press)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    box.pack_start(toolbar(), False, False, 0)
    box.pack_start(area, True, True, 0)
    win.add(box)


BOXES = [
    ("ALPHA", 40, 40), ("BRAVO", 240, 40), ("CHARLIE", 440, 40),
    ("DELTA", 640, 280), ("ECHO", 40, 280), ("FOXTROT", 240, 280),
    ("GOLF", 440, 280), ("HOTEL", 640, 40),
]
BOX_W, BOX_H = 150, 90


def build_board(win, state):
    def draw(cr):
        cr.select_font_face("Sans")
        cr.set_font_size(18)
        for name, x, y in BOXES:
            cr.set_source_rgb(0.85, 0.92, 1.0)
            cr.rectangle(x, y, BOX_W, BOX_H)
            cr.fill_preserve()
            cr.set_source_rgb(0.2, 0.3, 0.5)
            cr.set_line_width(2)
            cr.stroke()
            ext = cr.text_extents(name)
            cr.move_to(x + (BOX_W - ext.width) / 2, y + BOX_H / 2 + ext.height / 2)
            cr.show_text(name)

    def hit(px, py):
        for name, x, y in BOXES:
            if x <= px <= x + BOX_W and y <= py <= y + BOX_H:
                return name
        return None

    canvas_window(win, state, draw, hit)


SHAPES = [
    # (name, kind, colour, cx, cy, size)
    ("red square", "square", (0.9, 0.1, 0.1), 120, 120, 50),
    ("blue square", "square", (0.1, 0.3, 0.9), 320, 330, 50),
    ("green triangle", "triangle", (0.1, 0.7, 0.2), 520, 120, 60),
    ("purple circle", "circle", (0.6, 0.2, 0.8), 720, 330, 50),
    ("red circle", "circle", (0.9, 0.1, 0.1), 560, 360, 45),
    ("orange triangle", "triangle", (1.0, 0.55, 0.0), 160, 360, 60),
    ("blue circle", "circle", (0.1, 0.3, 0.9), 330, 130, 45),
]


def build_shapes(win, state):
    def draw(cr):
        for _name, kind, (r, g, b), cx, cy, s in SHAPES:
            cr.set_source_rgb(r, g, b)
            if kind == "circle":
                cr.arc(cx, cy, s, 0, 2 * math.pi)
            elif kind == "square":
                cr.rectangle(cx - s, cy - s, 2 * s, 2 * s)
            else:
                cr.move_to(cx, cy - s)
                cr.line_to(cx + s, cy + s * 0.8)
                cr.line_to(cx - s, cy + s * 0.8)
                cr.close_path()
            cr.fill()

    def hit(px, py):
        for name, kind, _c, cx, cy, s in SHAPES:
            if kind == "circle" and math.hypot(px - cx, py - cy) <= s:
                return name
            if kind != "circle" and cx - s <= px <= cx + s and cy - s <= py <= cy + s:
                return name
        return None

    canvas_window(win, state, draw, hit)


# --------------------------------------------------------------------------
# orders

ORDER_NUMBER = "58213-QX"


def build_orders(win, state):
    state.set("submitted", [])
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.set_border_width(12)
    notice = Gtk.DrawingArea()
    notice.set_size_request(520, 90)

    def on_draw(_a, cr):
        cr.set_source_rgb(1.0, 0.97, 0.85)
        cr.paint()
        cr.set_source_rgb(0.2, 0.2, 0.2)
        cr.select_font_face("Sans")
        cr.set_font_size(16)
        cr.move_to(16, 34)
        cr.show_text("Notice board")
        cr.set_font_size(22)
        cr.move_to(16, 70)
        cr.show_text("Today's order number: " + ORDER_NUMBER)

    notice.connect("draw", on_draw)
    box.pack_start(notice, False, False, 0)
    grid = Gtk.Grid(row_spacing=8, column_spacing=12)
    field = Gtk.Entry()
    labelled(grid, 0, "Order number", field)
    box.pack_start(grid, False, False, 0)
    buttons = Gtk.Box(spacing=8)
    clear = Gtk.Button(label="Clear")
    submit = Gtk.Button(label="Submit")
    clear.connect("clicked", lambda _b: field.set_text(""))

    def on_submit(_b):
        value = field.get_text()
        dlg = Gtk.Dialog(title="Confirm", transient_for=win, modal=True)
        dlg.get_content_area().pack_start(
            Gtk.Label(label="Submit order %s?" % value), False, False, 8)
        dlg.add_button("Cancel", Gtk.ResponseType.CANCEL)
        dlg.add_button("Confirm", Gtk.ResponseType.OK)

        def on_response(d, response):
            if response == Gtk.ResponseType.OK:
                state.append("submitted", value)
            d.destroy()

        dlg.connect("response", on_response)
        dlg.show_all()

    submit.connect("clicked", on_submit)
    buttons.pack_start(clear, False, False, 0)
    buttons.pack_start(submit, False, False, 0)
    box.pack_start(buttons, False, False, 0)
    win.add(box)
    win.set_default_size(560, 260)


# --------------------------------------------------------------------------
# counter: all painted

COUNTER_BUTTONS = [("PLUS", 60), ("MINUS", 230), ("DONE", 400)]
COUNTER_BUTTON_Y, COUNTER_BUTTON_W, COUNTER_BUTTON_H = 260, 140, 60


def build_counter(win, state):
    count = {"n": 0}
    state.set("count", 0)
    state.set("done", None)
    area = Gtk.DrawingArea()
    area.set_size_request(600, 380)
    area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)

    def on_draw(_a, cr):
        cr.set_source_rgb(1, 1, 1)
        cr.paint()
        cr.select_font_face("Sans")
        cr.set_source_rgb(0.1, 0.1, 0.1)
        cr.set_font_size(26)
        cr.move_to(60, 70)
        cr.show_text("Score keeper")
        cr.set_font_size(40)
        cr.move_to(60, 170)
        cr.show_text("Count: %d" % count["n"])
        cr.set_font_size(24)
        for name, x in COUNTER_BUTTONS:
            cr.set_source_rgb(0.85, 0.92, 1.0)
            cr.rectangle(x, COUNTER_BUTTON_Y, COUNTER_BUTTON_W, COUNTER_BUTTON_H)
            cr.fill_preserve()
            cr.set_source_rgb(0.2, 0.3, 0.5)
            cr.set_line_width(2)
            cr.stroke()
            ext = cr.text_extents(name)
            cr.move_to(x + (COUNTER_BUTTON_W - ext.width) / 2,
                       COUNTER_BUTTON_Y + COUNTER_BUTTON_H / 2 + ext.height / 2)
            cr.show_text(name)

    def on_press(_a, event):
        for name, x in COUNTER_BUTTONS:
            if x <= event.x <= x + COUNTER_BUTTON_W and \
                    COUNTER_BUTTON_Y <= event.y <= COUNTER_BUTTON_Y + COUNTER_BUTTON_H:
                if name == "PLUS":
                    count["n"] += 1
                elif name == "MINUS":
                    count["n"] -= 1
                else:
                    state.set("done", count["n"])
                state.set("count", count["n"])
                area.queue_draw()

    area.connect("draw", on_draw)
    area.connect("button-press-event", on_press)
    win.add(area)


BUILDERS = {
    "form": build_form,
    "table": build_table,
    "board": build_board,
    "shapes": build_shapes,
    "orders": build_orders,
    "counter": build_counter,
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--scenario", required=True, choices=sorted(BUILDERS))
    ap.add_argument("--state", default="")
    args = ap.parse_args()
    title = TITLES[args.scenario]
    # The app's name, as list_apps shows it: the window's title.
    GLib.set_prgname(title)
    GLib.set_application_name(title)
    # A steady caret, so pictures don't change between two looks.
    Gtk.Settings.get_default().set_property("gtk-cursor-blink", False)
    state = State(args.state)
    win = Gtk.Window(title=title)
    win.move(0, 0)
    BUILDERS[args.scenario](win, state)
    win.connect("destroy", Gtk.main_quit)
    win.show_all()
    Gtk.main()


if __name__ == "__main__":
    main()
