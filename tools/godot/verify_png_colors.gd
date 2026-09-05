extends SceneTree

func fail(message: String) -> void:
    push_error(message)
    quit(1)

func _initialize() -> void:
    for item in [["res://art/color.png", Vector2i(3840, 2160)], ["res://art/preview.png", Vector2i(1, 1)]]:
        var path: String = item[0]
        var direct := Image.new()
        if direct.load_png_from_buffer(FileAccess.get_file_as_bytes(path)) != OK or direct.get_size() != item[1]:
            fail("PNG decode or dimensions failed: " + path)
            return
        var pixel := direct.get_pixel(0, 0)
        print("godot-color-direct path=", path, " format=", direct.get_format(), " size=", direct.get_size(), " rgba8=", [pixel.r8, pixel.g8, pixel.b8, pixel.a8])
        var expected := Color8(255, 188, 137, 128)
        for channel in range(4):
            if abs(pixel[channel] - expected[channel]) > 1.0 / 255.0:
                fail("PNG color/alpha differs from independent sRGB expectation")
                return
        var texture := ResourceLoader.load(path) as Texture2D
        if texture == null or texture.get_size() != Vector2(item[1]):
            fail("Editor-imported texture failed: " + path)
            return
        var image := texture.get_image()
        if image == null:
            fail("Imported texture has no decoded image")
            return
        var imported := image.get_pixel(0, 0)
        print("godot-color-import path=", path, " format=", image.get_format(), " rgba8=", [imported.r8, imported.g8, imported.b8, imported.a8])
        for channel in range(4):
            if abs(imported[channel] - expected[channel]) > 1.0 / 255.0:
                fail("Imported texture color/alpha differs from independent sRGB expectation")
                return
    if OS.get_cmdline_user_args().has("--native-gradient"):
        var native := ResourceLoader.load("res://art/async-edit-scratch.png") as Texture2D
        if native == null or native.get_size() != Vector2(129, 65):
            fail("Native saved texture is missing or has wrong dimensions")
            return
        var native_image := native.get_image()
        if native_image == null:
            fail("Native saved texture has no decoded image")
            return
        # Independent linear-tile oracle at x=60: [1,72,102,126].
        # Compare the imported visible sRGB colors, allowing one encoded byte.
        for probe in [[Vector2i(0, 32), Color8(28, 199, 232, 255)], [Vector2i(60, 32), Color8(22, 199, 232, 126)]]:
            var actual := native_image.get_pixelv(probe[0])
            var expected: Color = probe[1]
            print("godot-native-gradient at=", probe[0], " rgba8=", [actual.r8, actual.g8, actual.b8, actual.a8])
            for channel in range(4):
                if abs(actual[channel] - expected[channel]) > 1.0 / 255.0:
                    fail("Editor cache differs from native gradient color/alpha")
                    return
        if native_image.get_pixel(128, 32).a != 0.0:
            fail("Transparent gradient endpoint became opaque")
            return
        print("godot-native-gradient status=passed source=existing-editor-import-cache")
    print("godot-color-acceptance status=passed decode=png16-and-preview8 import=editor-resource pixels=first-nonprimary-translucent headless=true watcher=false")
    quit(0)
