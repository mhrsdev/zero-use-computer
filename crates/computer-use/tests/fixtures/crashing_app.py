#!/usr/bin/env python3
"""A fake accessible app that quits, as a crash would, on the first question
it is asked (as Qt apps up to 6.9 did on `Properties.GetAll`). The live Qt
test runs it in a loop, as systemd restarts a crashed compositor, and
checks that the server asks a program that quit while answering no more
questions for a while: one crash, not one per restart.

  crashing_app.py <file to note each crash in>
"""
import os
import sys

from gi.repository import Gio, GLib

s = Gio.bus_get_sync(Gio.BusType.SESSION)
addr = s.call_sync("org.a11y.Bus", "/org/a11y/bus", "org.a11y.Bus", "GetAddress", None, GLib.VariantType("(s)"), 0, -1).unpack()[0]
c = Gio.DBusConnection.new_for_address_sync(addr, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION)
reg = c.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetNameOwner", GLib.Variant("(s)", ("org.a11y.atspi.Registry",)), GLib.VariantType("(s)"), 0, -1).unpack()[0]
me = c.get_unique_name()
def filt(conn, msg, incoming):
    if incoming and msg.get_message_type() == Gio.DBusMessageType.METHOD_CALL and msg.get_destination() == me and msg.get_sender() != reg:
        with open(sys.argv[1], "a") as f: f.write(f"died on {msg.get_interface()}.{msg.get_member()}\n")
        os._exit(3)
    return msg
c.add_filter(filt)
c.call_sync("org.a11y.atspi.Registry", "/org/a11y/atspi/accessible/root", "org.a11y.atspi.Socket", "Embed", GLib.Variant("((so))", ((me, "/org/a11y/atspi/accessible/root"),)), None, 0, -1)
GLib.MainLoop().run()
