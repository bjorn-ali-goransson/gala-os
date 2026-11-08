#version 450
// vk_test's triangle: three corners in clip space, no vertex buffer.
void main() {
    vec2 corners[3] = vec2[](vec2(-0.8, 0.8), vec2(0.8, 0.8), vec2(0.0, -0.8));
    gl_Position = vec4(corners[gl_VertexIndex], 0.0, 1.0);
}
