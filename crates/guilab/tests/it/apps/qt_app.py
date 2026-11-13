#!/usr/bin/env python3
"""Qt6 test app. Args: --menu (pop up a QMenu near the bottom-right corner)."""
import sys
from PySide6.QtCore import QTimer, QPoint
from PySide6.QtWidgets import QApplication, QLabel, QMainWindow, QMenu

app = QApplication(sys.argv[:1])
win = QMainWindow()
win.setWindowTitle("guilab qt test")
win.setCentralWidget(QLabel("qt main window"))
win.resize(400, 300)
win.show()
menu = QMenu(win)
for i in range(1, 6):
    menu.addAction(f"Qt item {i}")
if "--menu" in sys.argv:
    QTimer.singleShot(500, lambda: menu.popup(win.mapToGlobal(QPoint(win.width() - 20, win.height() - 20))))
sys.exit(app.exec())
