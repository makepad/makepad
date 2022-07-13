function watchFileChange() {
    var req = new XMLHttpRequest()
    req.timeout = 60000
    req.addEventListener("error", function() {
        
        setTimeout(function() {
            location.href = location.href
        }, 500)
    })
    req.responseType = 'text'
    req.addEventListener("load", function() {
        if (req.status === 201) return watchFileChange();
        if (req.status === 200) {
            if (req.response == ""){
                return
            }
            var msg = JSON.parse(req.response);
            if (msg.type == "file_change") {
                location.href = location.href
            }
            if (msg.type == "build_start") {
                let note = "Rebuilding application..."
                if (document.title != note) {
                    document.title = note;
                    console.log(note);
                }
                watchFileChange();
            }
        }
    })
    req.open("GET", "/$watch?" + ('' + Math.random()).slice(2))
    req.send()
}
watchFileChange()
