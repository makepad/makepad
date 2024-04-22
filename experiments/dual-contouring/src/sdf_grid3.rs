use crate::lerp::Lerp;
use crate::sdf3::Sdf3;
use crate::vector3::Vector3;

pub struct SdfGrid3 {
    pub root: Node,
    pub min: Vector3,
    pub max: Vector3,
}

impl SdfGrid3 {
    pub fn from_sdf<S: Sdf3>(sdf: &S, min: Vector3, max: Vector3, max_depth: usize) -> SdfGrid3 {
        let p000 = Vector3 {
            x: min.x,
            y: min.y,
            z: min.z,
        };
        let p001 = Vector3 {
            x: max.x,
            y: min.y,
            z: min.z,
        };
        let p010 = Vector3 {
            x: min.x,
            y: max.y,
            z: min.z,
        };
        let p011 = Vector3 {
            x: max.x,
            y: max.y,
            z: min.z,
        };
        let p100 = Vector3 {
            x: min.x,
            y: min.y,
            z: max.z,
        };
        let p101 = Vector3 {
            x: max.x,
            y: min.y,
            z: max.z,
        };
        let p110 = Vector3 {
            x: min.x,
            y: max.y,
            z: max.z,
        };
        let p111 = Vector3 {
            x: max.x,
            y: max.y,
            z: max.z,
        };

        let d000 = sdf.distance(p000);
        let d001 = sdf.distance(p001);
        let d010 = sdf.distance(p010);
        let d011 = sdf.distance(p011);
        let d100 = sdf.distance(p100);
        let d101 = sdf.distance(p101);
        let d110 = sdf.distance(p110);
        let d111 = sdf.distance(p111);

        let n000 = sdf.normal(p000);
        let n001 = sdf.normal(p001);
        let n010 = sdf.normal(p010);
        let n011 = sdf.normal(p011);
        let n100 = sdf.normal(p100);
        let n101 = sdf.normal(p101);
        let n110 = sdf.normal(p110);
        let n111 = sdf.normal(p111);

        SdfGrid3 {
            root: Node::from_sdf(
                sdf,
                min,
                max,
                [d000, d001, d010, d011, d100, d101, d110, d111],
                [n000, n001, n010, n011, n100, n101, n110, n111],
                0,
                max_depth,
            ),
            min,
            max,
        }
    }
}

impl Sdf3 for SdfGrid3 {
    fn distance(&self, p: Vector3) -> f32 {
        self.root.distance(p, self.min, self.max)
    }

    fn normal(&self, p: Vector3) -> Vector3 {
        self.root.normal(p, self.min, self.max)
    }
}

pub enum Node {
    Leaf {
        distances: [f32; 8],
        normals: [Vector3; 8],
    },
    Branch {
        children: Box<[Node; 8]>,
    },
}

impl Node {
    fn from_sdf<S: Sdf3>(
        sdf: &S,
        min: Vector3,
        max: Vector3,
        distances: [f32; 8],
        normals: [Vector3; 8],
        depth: usize,
        max_depth: usize,
    ) -> Node {
        if depth == max_depth {
            Node::Leaf { distances, normals }
        } else {
            let mid = min.lerp(max, 0.5);

            let p001 = Vector3 {
                x: mid.x,
                y: min.y,
                z: min.z,
            };
            let p010 = Vector3 {
                x: min.x,
                y: mid.y,
                z: min.z,
            };
            let p011 = Vector3 {
                x: mid.x,
                y: mid.y,
                z: min.z,
            };
            let p012 = Vector3 {
                x: max.x,
                y: mid.y,
                z: min.z,
            };
            let p021 = Vector3 {
                x: mid.x,
                y: max.y,
                z: min.z,
            };
            let p100 = Vector3 {
                x: min.x,
                y: min.y,
                z: mid.z,
            };
            let p101 = Vector3 {
                x: mid.x,
                y: min.y,
                z: mid.z,
            };
            let p102 = Vector3 {
                x: max.x,
                y: min.y,
                z: mid.z,
            };
            let p110 = Vector3 {
                x: min.x,
                y: mid.y,
                z: mid.z,
            };
            let p111 = Vector3 {
                x: mid.x,
                y: mid.y,
                z: mid.z,
            };
            let p112 = Vector3 {
                x: max.x,
                y: mid.y,
                z: mid.z,
            };
            let p120 = Vector3 {
                x: min.x,
                y: max.y,
                z: mid.z,
            };
            let p121 = Vector3 {
                x: mid.x,
                y: max.y,
                z: mid.z,
            };
            let p122 = Vector3 {
                x: max.x,
                y: max.y,
                z: mid.z,
            };
            let p201 = Vector3 {
                x: mid.x,
                y: min.y,
                z: max.z,
            };
            let p210 = Vector3 {
                x: min.x,
                y: mid.y,
                z: max.z,
            };
            let p211 = Vector3 {
                x: mid.x,
                y: mid.y,
                z: max.z,
            };
            let p212 = Vector3 {
                x: max.x,
                y: mid.y,
                z: max.z,
            };
            let p221 = Vector3 {
                x: mid.x,
                y: max.y,
                z: max.z,
            };

            let [d000, d002, d020, d022, d200, d202, d220, d222] = distances;
            let d001 = sdf.distance(p001);
            let d010 = sdf.distance(p010);
            let d011 = sdf.distance(p011);
            let d012 = sdf.distance(p012);
            let d021 = sdf.distance(p021);
            let d100 = sdf.distance(p100);
            let d101 = sdf.distance(p101);
            let d102 = sdf.distance(p102);
            let d110 = sdf.distance(p110);
            let d111 = sdf.distance(p111);
            let d112 = sdf.distance(p112);
            let d120 = sdf.distance(p120);
            let d121 = sdf.distance(p121);
            let d122 = sdf.distance(p122);
            let d201 = sdf.distance(p201);
            let d210 = sdf.distance(p210);
            let d211 = sdf.distance(p211);
            let d212 = sdf.distance(p212);
            let d221 = sdf.distance(p221);

            let [n000, n002, n020, n022, n200, n202, n220, n222] = normals;
            let n001 = sdf.normal(p001);
            let n010 = sdf.normal(p010);
            let n011 = sdf.normal(p011);
            let n012 = sdf.normal(p012);
            let n021 = sdf.normal(p021);
            let n100 = sdf.normal(p100);
            let n101 = sdf.normal(p101);
            let n102 = sdf.normal(p102);
            let n110 = sdf.normal(p110);
            let n111 = sdf.normal(p111);
            let n112 = sdf.normal(p112);
            let n120 = sdf.normal(p120);
            let n121 = sdf.normal(p121);
            let n122 = sdf.normal(p122);
            let n201 = sdf.normal(p201);
            let n210 = sdf.normal(p210);
            let n211 = sdf.normal(p211);
            let n212 = sdf.normal(p212);
            let n221 = sdf.normal(p221);

            Node::Branch {
                children: Box::new([
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: min.x,
                            y: min.y,
                            z: min.z,
                        },
                        Vector3 {
                            x: mid.x,
                            y: mid.y,
                            z: mid.z,
                        },
                        [d000, d001, d010, d011, d100, d101, d110, d111],
                        [n000, n001, n010, n011, n100, n101, n110, n111],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: mid.x,
                            y: min.y,
                            z: min.z,
                        },
                        Vector3 {
                            x: max.x,
                            y: mid.y,
                            z: mid.z,
                        },
                        [d001, d002, d011, d012, d101, d102, d111, d112],
                        [n001, n002, n011, n012, n101, n102, n111, n112],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: min.x,
                            y: mid.y,
                            z: min.z,
                        },
                        Vector3 {
                            x: mid.x,
                            y: max.y,
                            z: mid.z,
                        },
                        [d010, d011, d020, d021, d110, d111, d120, d121],
                        [n010, n011, n020, n021, n110, n111, n120, n121],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: mid.x,
                            y: mid.y,
                            z: min.z,
                        },
                        Vector3 {
                            x: max.x,
                            y: max.y,
                            z: mid.z,
                        },
                        [d011, d012, d021, d022, d111, d112, d121, d122],
                        [n011, n012, n021, n022, n111, n112, n121, n122],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: min.x,
                            y: min.y,
                            z: mid.z,
                        },
                        Vector3 {
                            x: mid.x,
                            y: mid.y,
                            z: max.z,
                        },
                        [d100, d101, d110, d111, d200, d201, d210, d211],
                        [n100, n101, n110, n111, n200, n201, n210, n211],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: mid.x,
                            y: min.y,
                            z: mid.z,
                        },
                        Vector3 {
                            x: max.x,
                            y: mid.y,
                            z: max.z,
                        },
                        [d101, d102, d111, d112, d201, d202, d211, d212],
                        [n101, n102, n111, n112, n201, n202, n211, n212],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: min.x,
                            y: mid.y,
                            z: mid.z,
                        },
                        Vector3 {
                            x: mid.x,
                            y: max.y,
                            z: max.z,
                        },
                        [d110, d111, d120, d121, d210, d211, d220, d221],
                        [n110, n111, n120, n121, n210, n211, n220, n221],
                        depth + 1,
                        max_depth,
                    ),
                    Node::from_sdf(
                        sdf,
                        Vector3 {
                            x: mid.x,
                            y: mid.y,
                            z: mid.z,
                        },
                        Vector3 {
                            x: max.x,
                            y: max.y,
                            z: max.z,
                        },
                        [d111, d112, d121, d122, d211, d212, d221, d222],
                        [n111, n112, n121, n122, n211, n212, n221, n222],
                        depth + 1,
                        max_depth,
                    ),
                ]),
            }
        }
    }

    fn distance(&self, p: Vector3, min: Vector3, max: Vector3) -> f32 {
        match self {
            &Node::Leaf {
                distances: [d000, d002, d020, d022, d200, d202, d220, d222],
                ..
            } => {
                let t = Vector3 {
                    x: (p.x - min.x) / (max.x - min.x),
                    y: (p.y - min.y) / (max.y - min.y),
                    z: (p.z - min.z) / (max.z - min.z),
                };

                let d001 = d000.lerp(d002, t.x);
                let d021 = d020.lerp(d022, t.x);
                let d201 = d200.lerp(d202, t.x);
                let d221 = d220.lerp(d222, t.x);
                let d011 = d001.lerp(d021, t.y);
                let d211 = d201.lerp(d221, t.y);
                let d111 = d011.lerp(d211, t.z);

                d111
            }
            Node::Branch { children } => {
                let mid = min.lerp(max, 0.5);

                if p.z < mid.z {
                    if p.y < mid.y {
                        if p.x < mid.x {
                            children[0b000].distance(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                            )
                        } else {
                            children[0b001].distance(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                            )
                        }
                    } else {
                        if p.x < mid.x {
                            children[0b010].distance(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: mid.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: max.y,
                                    z: mid.z,
                                },
                            )
                        } else {
                            children[0b011].distance(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: mid.z,
                                },
                            )
                        }
                    }
                } else {
                    if p.y < mid.y {
                        if p.x < mid.x {
                            children[0b100].distance(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: max.z,
                                },
                            )
                        } else {
                            children[0b101].distance(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: min.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: mid.y,
                                    z: max.z,
                                },
                            )
                        }
                    } else {
                        if p.x < mid.x {
                            children[0b110].distance(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: max.y,
                                    z: max.z,
                                },
                            )
                        } else {
                            children[0b111].distance(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: max.z,
                                },
                            )
                        }
                    }
                }
            }
        }
    }

    fn normal(&self, p: Vector3, min: Vector3, max: Vector3) -> Vector3 {
        match self {
            &Node::Leaf {
                normals: [n000, n002, n020, n022, n200, n202, n220, n222],
                ..
            } => {
                let t = Vector3 {
                    x: (p.x - min.x) / (max.x - min.x),
                    y: (p.y - min.y) / (max.y - min.y),
                    z: (p.z - min.z) / (max.z - min.z),
                };

                let n001 = n000.lerp(n002, t.x);
                let n021 = n020.lerp(n022, t.x);
                let n201 = n200.lerp(n202, t.x);
                let n221 = n220.lerp(n222, t.x);
                let n011 = n001.lerp(n021, t.y);
                let n211 = n201.lerp(n221, t.y);
                let n111 = n011.lerp(n211, t.z);

                n111
            }
            Node::Branch { children } => {
                let mid = min.lerp(max, 0.5);

                if p.z < mid.z {
                    if p.y < mid.y {
                        if p.x < mid.x {
                            children[0b000].normal(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                            )
                        } else {
                            children[0b001].normal(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: min.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                            )
                        }
                    } else {
                        if p.x < mid.x {
                            children[0b010].normal(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: mid.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: max.y,
                                    z: mid.z,
                                },
                            )
                        } else {
                            children[0b011].normal(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: min.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: mid.z,
                                },
                            )
                        }
                    }
                } else {
                    if p.y < mid.y {
                        if p.x < mid.x {
                            children[0b100].normal(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: min.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: max.z,
                                },
                            )
                        } else {
                            children[0b101].normal(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: min.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: mid.y,
                                    z: max.z,
                                },
                            )
                        }
                    } else {
                        if p.x < mid.x {
                            children[0b110].normal(
                                p,
                                Vector3 {
                                    x: min.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: mid.x,
                                    y: max.y,
                                    z: max.z,
                                },
                            )
                        } else {
                            children[0b111].normal(
                                p,
                                Vector3 {
                                    x: mid.x,
                                    y: mid.y,
                                    z: mid.z,
                                },
                                Vector3 {
                                    x: max.x,
                                    y: max.y,
                                    z: max.z,
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}
