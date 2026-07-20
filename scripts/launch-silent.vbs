' launch-silent.vbs — starts nudge-svc (tray, hidden, only if not already running) then nudge-app (GUI); run via wscript for a zero-console launch, no rebuild. Resource cost: two Start calls, no resident process of its own.
Option Explicit
Dim fso, sh, scriptDir, root, svc, app, procs
Set fso = CreateObject("Scripting.FileSystemObject")
Set sh  = CreateObject("WScript.Shell")

scriptDir = fso.GetParentFolderName(WScript.ScriptFullName)
root = fso.GetParentFolderName(scriptDir)
svc = root & "\target\release\nudge-svc.exe"
app = root & "\crates\nudge-app\src-tauri\target\release\nudge-app.exe"

' Start the resident tray service only if it isn't already running.
Set procs = GetObject("winmgmts:\\.\root\cimv2").ExecQuery( _
    "Select ProcessId from Win32_Process Where Name = 'nudge-svc.exe'")
If procs.Count = 0 And fso.FileExists(svc) Then
    sh.Run """" & svc & """", 0, False   ' hidden, async
End If

' Launch the GUI app (normal window, async).
If fso.FileExists(app) Then
    sh.Run """" & app & """", 1, False
End If
