If ([bool](schtasks /query /fo list | Select-String -pattern "TaskName:\s+\\{{TASK_NAME}}" -quiet)) {
	Start-Process C:\Windows\System32\schtasks.exe "/delete /tn {{TASK_NAME}} /f" -Verb runas -Wait
	echo "Autostart has been unscheduled."
	break
}
