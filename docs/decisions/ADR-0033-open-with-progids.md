# ADR-0033: Register current Shell Open With ProgIDs

- Date: 2026-09-06
- Status: Accepted for registration ownership; Explorer activation gate remains open

## Context

The development installer registered Applications/NyatiDraw.exe with SupportedTypes
and a legacy .png/OpenWithList child. Actual Windows 11 Explorer acceptance did not
show NyatiDraw in the PNG cascade; double-clicking the scratch .ntdr opened an app
picker without NyatiDraw in its alphabetical position. Registry existence and prior
positional executable probes had not established Explorer acceptance.

Microsoft's [Open With registration](https://learn.microsoft.com/en-us/windows/win32/shell/how-to-include-an-application-on-the-open-with-dialog-box)
specifies OpenWithProgIds for each supported extension and describes OpenWithList
as a pre-Windows-XP legacy mechanism. This is a registration deficiency; fixing it
alone has not yet resolved the observed Explorer behavior on this host.

## Decision

Add an empty REG_SZ NyatiDraw.Project.1 value to HKCU/Software/Classes under both
.png/OpenWithProgIds and .ntdr/OpenWithProgIds. Keep the existing versioned ProgID,
quoted absolute command, application name, SupportedTypes, and owned legacy key.
No PNG default or UserChoice is written. Existing exact owned legacy registrations
can upgrade additively; partial new registration and a colliding named value fail
before bundle replacement. The PNG list is shared, so other names and types are
preserved. Uninstall removes only our exact named value while the owning project
marker and command match; it removes the list key only when empty.

Both install and uninstall use SHCNF_FLUSH with SHCNE_ASSOCCHANGED so notification
delivery completes before success is printed, following the
[SHChangeNotify contract](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/nf-shlobj_core-shchangenotify).
No dependencies or automated UI/registry mock tests were added.

## Evidence and limits

Windows 11 Home build 26200, Core Ultra 7 155H, Intel Arc integrated; installed
DX 0.7.9 release binary built at 07ef0c7, SHA-256
96BBFA0ECEA62D38F44574B627088AEDF5001525D372259D839E8DE0A0556140.

Scratch records are under target/explorer-openwith:

- registration-upgrade/repeat/flush.log: legacy upgrade, exact new-shape update.
- registration-acceptance.json and shared-value-*.log: update/uninstall/reinstall
  preserve a deliberately added foreign PNG list value and all pre-existing names;
  final PNG list and UserChoice exact equality, binary hash equality, project and
  PNG byte equality. The scratch foreign value was removed after verification.
- collision-rejection.log: a changed own ProgID value rejects update before binary
  replacement; the colliding value itself is not overwritten.
- query-associations.log: fresh AssocQueryString .ntdr and versioned ProgID command,
  executable, and friendly name resolve successfully. The application-key query in
  that exploratory log used extension-style flags and is not application proof.
- Shell launch by scratch .ntdr filename (not executable path) created NyatiDraw
  PID 25020; installed UI displayed original page, signed outside-page ink and
  locked translucent content. Normal UI close released storage.
- shell-open-verify.log: stage 1, snapshot 1, history 1, complete tile/tree/page and
  decoded PNG comparison succeeds after that process exited.

The existing Explorer window still showed a generic NTDR type and omitted
NyatiDraw from the PNG cascade after registration changes; F5 did not change the
type display. This is unresolved. Read-only process checks showed an unpackaged
PowerShell process (GetCurrentPackageFullName = APPMODEL_ERROR_NO_PACKAGE) in the
same session as Explorer; package virtualization is not established as the cause.
No Explorer restart, PC restart, download process/file inspection, or PNG default
change was performed. The actual Explorer draw/erase/Undo/Redo/Save/Close/reopen
acceptance remains open. New-process Shell success is a narrower result.
