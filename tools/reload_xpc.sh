PLIST=~/"Library/LaunchAgents/dev.makepad.xpc.plist"
echo "<?xml version=\"1.0\" encoding=\"UTF-8\"?>">$PLIST 
echo "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">">>$PLIST 
echo "<plist version=\"1.0\">">>$PLIST
echo "<dict>">>$PLIST 
echo "  <key>Label</key>">>$PLIST 
echo "  <string>dev.makepad.metalxpc</string>">>$PLIST 
echo "  <key>Program</key>">>$PLIST 
echo "  <string>$PWD/target/debug/makepad_studio</string>">>$PLIST 
echo "  <key>ProgramArguments</key>">>$PLIST
echo "  <array>">>$PLIST
echo "    <string>$PWD/target/debug/makepad_studio</string>">>$PLIST
echo "    <string>--metal-xpc</string>">>$PLIST
echo "  </array>">>$PLIST
echo "    <key>MachServices</key>">>$PLIST 
echo "    <dict>">>$PLIST 
echo "        <key>dev.makepad.metalxpc</key>">>$PLIST 
echo "        <true/>">>$PLIST 
echo "    </dict>">>$PLIST 
echo "</dict>">>$PLIST 
echo "</plist>">>$PLIST
echo "">>$PLIST

launchctl unload $PLIST  
launchctl load $PLIST

#$PWD/target/debug/makepad_studio --render-to=0
