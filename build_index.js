var fs = require('fs');

function readRecur(path, name){
    var files = fs.readdirSync(path);
    var ret = {name:name,open:false,folders:[], files:[]};
    for(file of files){
        let new_file = path + '/' + file
        if (fs.statSync(new_file).isDirectory()) {
            if( file == 'target') continue;
            let sub = readRecur(new_file, file);
            if(sub.folders.length > 0 || sub.files.length > 0){ // prune empty dirs
                ret.folders.push(sub)
            }
        }
        else{
            if(!file.match(/\.rs/)) continue;
            ret.files.push({name:file})
        }
    }
    return ret
}
let tree = readRecur(".","");
tree.open = true;
tree.folders[0].open = true;
tree.folders[0].folders[0].open = true;
tree.folders[2].open = true;
tree.folders[3].open = true;
//tree.folders[3].folders[0].open = true;
var data_in = fs.readFileSync('./index.json');
var data_out = JSON.stringify(tree);
if(data_in != data_out){
    fs.writeFileSync('./index.json', data_out);
}
