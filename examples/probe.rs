//! cargo run --release --example probe -- model.onnx H W
use std::time::Instant;
use tract_onnx::prelude::*;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (h, w): (usize, usize) = (a[2].parse().unwrap(), a[3].parse().unwrap());
    let t = Instant::now();
    let m = tract_onnx::onnx().with_ignore_value_info(true).with_ignore_output_shapes(true).model_for_path(&a[1]).expect("parse");
    for i in m.input_outlets().unwrap() { println!("input {:?}", m.outlet_fact(*i).unwrap()); }
    let m = m.with_input_fact(0, InferenceFact::dt_shape(f32::datum_type(), tvec!(1, 3, h, w))).unwrap();
    let plan = match m.into_optimized() { Ok(p) => p, Err(e) => { println!("OPTIMIZE FAIL: {e:#}"); return; } };
    let plan = plan.into_runnable().unwrap();
    println!("loaded in {:?}", t.elapsed());
    let data: Vec<f32> = (0..3*h*w).map(|i| ((i % 97) as f32) / 97.0).collect();
    let inp = Tensor::from_shape(&[1, 3, h, w], &data).unwrap();
    let t = Instant::now();
    let n=std::thread::available_parallelism().map(|n|n.get()).unwrap_or(1);
    let ex=tract_linalg::multithread::Executor::multithread(n);
    let r=tract_linalg::multithread::multithread_tract_scope(ex,||plan.run(tvec!(inp.into())));
    match r {
        Ok(o) => println!("run ok in {:?}: out shape {:?}", t.elapsed(), o[0].shape()),
        Err(e) => println!("RUN FAIL: {e:#}"),
    }
}
