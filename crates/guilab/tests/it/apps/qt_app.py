#!/usr/bin/env python3
"""Qt6 test app: a line edit over a label. Right-click the label for a QMenu.
Prints events (flushed): `edit TEXT`, `menu N`, and once shown `pos NAME X Y W H`.
Args: --menu (pop up the QMenu near the bottom-right corner on its own; Qt needs an
input serial for that, so without input it doesn't open)."""
import sys
from PySide6.QtCore import QTimer, QPoint, Qt
from PySide6.QtWidgets import QApplication, QLabel, QLineEdit, QMainWindow, QMenu, QVBoxLayout, QWidget


def out(*a):
    print(*a, flush=True)


app = QApplication(sys.argv[:1])
win = QMainWindow()
win.setWindowTitle("guilab qt test")
central = QWidget()
lay = QVBoxLayout(central)
edit = QLineEdit()
edit.textChanged.connect(lambda t: out("edit", repr(t)))
lay.addWidget(edit)
label = QLabel("qt main window (right-click: menu)")
label.setAlignment(Qt.AlignCenter)
lay.addWidget(label, 1)
win.setCentralWidget(central)
win.resize(400, 300)
win.show()
menu = QMenu(win)
for i in range(1, 6):
    menu.addAction(f"Qt item {i}").triggered.connect(lambda _=False, i=i: out("menu", i))
label.setContextMenuPolicy(Qt.CustomContextMenu)
label.customContextMenuRequested.connect(lambda p: menu.popup(label.mapToGlobal(p)))


def report():
    # Once shown and laid out.
    if not (win.windowHandle() and win.windowHandle().isExposed()) or win.width() <= 1:
        QTimer.singleShot(30, report)
        return
    for name, w in (("edit", edit), ("label", label)):
        p = w.mapTo(win, QPoint(0, 0))
        out("pos", name, p.x(), p.y(), w.width(), w.height())
    out("ready")


QTimer.singleShot(30, report)
if "--menu" in sys.argv:
    QTimer.singleShot(500, lambda: menu.popup(win.mapToGlobal(QPoint(win.width() - 20, win.height() - 20))))
sys.exit(app.exec())
