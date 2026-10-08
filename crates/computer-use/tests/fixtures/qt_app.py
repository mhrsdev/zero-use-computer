#!/usr/bin/env python3
"""A small Qt 6 window for the live Linux test: a button, a text field and
a label, exposed through Qt's own AT-SPI bridge (not ATK).

Qt's bridge answers some calls differently from GTK's, and up to Qt 6.9 it
crashes on `org.freedesktop.DBus.Properties.GetAll` (it reads a second
argument that call doesn't have). The live test lists and reads this app,
then checks that it is still running.
"""

import os
import sys

# Qt's bridge starts only when accessibility is on; on a bare test bus
# nothing switches it on, so ask for it outright.
os.environ.setdefault("QT_LINUX_ACCESSIBILITY_ALWAYS_ON", "1")
os.environ.setdefault("QT_QPA_PLATFORM", "xcb")

from PyQt6.QtWidgets import (  # noqa: E402
    QApplication,
    QLabel,
    QLineEdit,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

app = QApplication(sys.argv)
app.setApplicationName("QtFixture")
win = QWidget()
win.setWindowTitle("Qt Fixture")
layout = QVBoxLayout(win)
label = QLabel("Ready")
field = QLineEdit()
field.setAccessibleName("Qt field")
button = QPushButton("Press me")
button.clicked.connect(lambda: label.setText("Pressed"))
for w in (label, field, button):
    layout.addWidget(w)
win.resize(320, 160)
win.show()
sys.exit(app.exec())
