


let fib = |n| if n <= 1 n else fib(n - 1) + fib(n - 2)
~fib(34);

let MyApp = App{
    $app_bar+:{
        title: "Talk to AI"
    }
    $body: +{
        TextInput{
            on_enter: ||{
                let stream = ai(this.text)
                this.clear()
                $body += Label{
                    text <=> stream.text
                }
            }
        }
    }
}

http.server(8080,{
    on_request:|req,res| res.write(200,"Working")
})
