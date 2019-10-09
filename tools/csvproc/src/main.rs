#[derive(Debug)]
struct Test{
    x:u32
}

fn main(){
   for i in 0..10000{
       let t=Test{x:i};
       println!("Testing logging log {:?}", t);
   }
}
