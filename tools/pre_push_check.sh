
echo "Checking Linux Direct stable"
MAKEPAD=linux_direct cargo +stable check -q -p makepad-example-ironfish --release --target=x86_64-unknown-linux-gnu --message-format=json
