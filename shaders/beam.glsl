// ABOUTME: Optional Ghostty transporter shader; inactive unless Beam signals a slow task.
// ABOUTME: Preserves text, selection and screen alpha; uses cursor color as an opt-in signal.
// Install with custom-shader = /absolute/path/to/beam.glsl, then BEAM_EFFECT=shader beam.

float beamMatch(vec3 color, vec3 marker) {
    // Ghostty may provide native or linear colors. Accept both representations.
    vec3 linearMarker = pow((marker + vec3(0.055)) / 1.055, vec3(2.4));
    float d = min(distance(color, marker), distance(color, linearMarker));
    return 1.0 - step(0.025, d);
}

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    vec2 uv = fragCoord / iResolution.xy;
    vec4 original = texture(iChannel0, uv);
    fragColor = original;
    float up = beamMatch(iCurrentCursorColor.rgb, vec3(11.0, 202.0, 225.0) / 255.0);
    float down = beamMatch(iCurrentCursorColor.rgb, vec3(169.0, 128.0, 238.0) / 255.0);
    float enabled = max(up, down);
    if (enabled < 0.5 || iFocus < 1) return;

    // Background pixels receive light; glyphs and selected cells keep their original pixels.
    float background = 1.0 - smoothstep(0.015, 0.09, distance(original.rgb, iBackgroundColor));
    if (background < 0.01) return;
    float direction = up > down ? 1.0 : -1.0;
    float x = uv.x * (iResolution.x / max(iResolution.y, 1.0));
    float shafts = pow(abs(sin(x * 17.0 + sin(x * 5.0) * 0.4)), 24.0);
    float wave = 0.5 + 0.5 * sin(uv.y * 14.0 - direction * iTime * 0.65 + x * 2.0);
    float envelope = pow(sin(3.14159265 * clamp(uv.x, 0.0, 1.0)), 2.0);
    float fade = smoothstep(0.0, 0.8, iTime - iTimeCursorChange);
    float amount = background * envelope * fade * shafts * (0.008 + wave * 0.022);
    // Mixing works on light themes too; never make the terminal more opaque.
    vec3 light = mix(vec3(0.0, 0.78, 0.77), vec3(0.66, 0.50, 0.93), down);
    fragColor.rgb = mix(original.rgb, light, amount);
    fragColor.a = original.a;
}
