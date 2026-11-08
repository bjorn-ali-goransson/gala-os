#version 450
// vk_test's triangle colour: (1.0, 0.8, 0.0), 0xffcc00 in UNORM.
layout(location = 0) out vec4 color;
void main() {
    color = vec4(1.0, 0.8, 0.0, 1.0);
}
