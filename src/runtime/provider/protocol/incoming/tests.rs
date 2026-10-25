use super::*;

#[test]
fn response_metadata_shares_the_frame_projection() {
    let frame = IncomingFrame::parse(json!({"id":1,"result":{
        "model":"model","data":vec![0; 4096]}}));
    let response = frame.result().expect("fixture has a result");
    assert!(Arc::ptr_eq(&frame.fields, &response.fields));
    let cloned = response.clone();
    assert!(Arc::ptr_eq(&response.fields, &cloned.fields));
    drop(frame);
    assert_eq!(response.model(), Some("model"));
    assert_eq!(response.data_is_empty(), Some(false));
    assert_eq!(
        cloned.into_raw()["data"].as_array().map(Vec::len),
        Some(4096)
    );
}
