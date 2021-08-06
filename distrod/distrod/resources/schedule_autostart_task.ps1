While ($true) {
    Start-Process C:\Windows\System32\schtasks.exe "/create /ru {{USER_NAME}} /tn {{TASK_NAME}} /xml {{TASK_FILE_WINDOWS_PATH}}" -Verb runas -Wait
    If ([bool](schtasks /query /fo list | Select-String -pattern "TaskName:\s+\\{{TASK_NAME}}" -quiet)) {
        echo "Enabling autostart has succeeded."
        break
    }
    If ($Host.UI.PromptForChoice("Error", "It seems the task has not been scheduled successfully. You may have typed a wrong password, or you may not have the necessary administrative privileges. Do you want to retry?", ("&Yes", "&No"), 0) -eq 1) {
        echo "Enabling autostart has been cancelled."
        exit 1
    }
}
