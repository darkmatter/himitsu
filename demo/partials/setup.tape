# Hidden setup only. The visible demo below is pure TUI.
Hide
Set TypingSpeed 1ms
Type "source demo/tui-demo-setup.sh && DEMO_NAME=main tui_demo_prepare"
Enter
Wait+Line />$/
Type "tui_demo_register_second_store"
Enter
Wait+Line />$/
Type "trap tui_demo_cleanup EXIT"
Enter
Type "mkdir -p target/vhs-out/screenshots"
Enter
Type "clear"
Enter
Sleep 300ms
Show
Set TypingSpeed 42ms
