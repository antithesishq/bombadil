use chromiumoxide::layout;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Serialize, Deserialize, Debug)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl From<layout::Point> for Point {
    fn from(val: layout::Point) -> Self {
        Point { x: val.x, y: val.y }
    }
}

impl From<Point> for layout::Point {
    fn from(val: Point) -> Self {
        layout::Point { x: val.x, y: val.y }
    }
}

impl Point {
    pub fn to_api(&self) -> bombadil_schema::Point {
        bombadil_schema::Point {
            x: self.x,
            y: self.y,
        }
    }
}
