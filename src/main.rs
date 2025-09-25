use warp::Filter;

#[tokio::main]
async fn main() {
    let index = warp::get()
        .and(warp::path::end())
        .and(warp::fs::file("www/static/index.html"));

    warp::serve(index).run(([0, 0, 0, 0], 3000)).await;
}
