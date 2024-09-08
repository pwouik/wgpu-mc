use glam::{ivec3, IVec3};

static VECTOR:[IVec3;6] = [
    ivec3(-1, 0, 0),
    ivec3(1, 0, 0),
    ivec3(0, -1, 0),
    ivec3(0, 1, 0),
    ivec3(0, 0, -1),
    ivec3(0, 0, 1),
];
#[derive(Debug,Clone,Copy)]
pub enum Direction {
    West=0,
    East=1,
    Down=2,
    Up=3,
    North=4,
    South=5,
}
impl Direction{
    pub fn to_vec(&self)->IVec3{
        VECTOR[*self as usize]
    }
    pub fn opposite(&self)->Self{
        match self {
            Self::West => Self::East,
            Self::East => Self::West,
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
        }
    }
}