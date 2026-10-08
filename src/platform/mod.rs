// Hardware- and OS-specific code lives behind traits in this module (window
// control, display mode, installers) so an x86 port swaps modules, not code.
// The traits arrive with the features that need them: Windows in M3,
// Installer in M4, Display in M5.
