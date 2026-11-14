# Flaky: input::gtk_input_and_clipboard "menu 2"

Fails about 1 in 3 runs (also before recent changes): clicking the second item of the GTK context menu right after the popup shows up in `tree` sometimes doesn't register, so "menu 2" never appears. Likely the popover isn't accepting input yet (map/animation) when the click lands. Fix in the test (wait for the popup to settle, e.g. a screenshot settle, before clicking) or, if apps generally drop clicks on just-mapped popups, make `input` wait for the popup's first frame.
