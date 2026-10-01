# ABOUTME: Validate the optional Ghostty shader with its documented uniforms.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
shader_tmp=$(mktemp -d)
trap 'rm -rf "$shader_tmp"' EXIT
cat > "$shader_tmp/beam.frag" <<'GLSL'
#version 330 core
uniform sampler2D iChannel0;
uniform vec3 iResolution;
uniform float iTime;
uniform vec4 iCurrentCursorColor;
uniform int iFocus;
uniform vec3 iBackgroundColor;
uniform float iTimeCursorChange;
out vec4 outputColor;
GLSL
cat "$root/shaders/beam.glsl" >> "$shader_tmp/beam.frag"
printf '\nvoid main() { mainImage(outputColor, gl_FragCoord.xy); }\n' >> "$shader_tmp/beam.frag"
glslangValidator -S frag "$shader_tmp/beam.frag"
