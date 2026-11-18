@receipt L3
qblock residual(x: i64[8]) -> i64[8] {
  config {
    model_id = 11;
    lane_width = 8;
    tile_size = 8;
    node_count = 2;
    seed = 7;
  }
  let h = dot(x, 3);
  let g = dot(h, 5);
  let r = gate(g, h, 9);
  emit r;
}
