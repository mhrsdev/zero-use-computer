//! A process that a host started (`--host`) serves no settings page, whatever
//! asks for one. This is its own test binary because the switch is for the
//! whole process.

#[test]
fn a_disabled_panel_is_neither_opened_nor_served() {
    computer_use::panel::disable();
    assert!(computer_use::panel::open(None, "").is_err());
    let mut said_ready = false;
    assert!(computer_use::panel::serve(None, false, |_| said_ready = true).is_err());
    assert!(
        !said_ready,
        "no address was given for a page that is not served"
    );
}
