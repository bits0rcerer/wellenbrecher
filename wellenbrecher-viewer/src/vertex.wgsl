struct VertexOutput {
  @builtin(position) Position : vec4<f32>,
  @location(0) fragUV : vec2<f32>,
}

@vertex
fn main(@location(0) position : vec2<f32>, @location(1) uv : vec2<f32>) -> VertexOutput {
  return VertexOutput(
      vec4<f32>((position - vec2<f32>(0.5)) * vec2<f32>(2.0), 0.0, 1.0),
      uv
   );
}

