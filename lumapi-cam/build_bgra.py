import PIL.Image
import os

icons = [
    'play.png',
    'pause.png',
    'rewind.png',
    'forward.png',
    'exit.png'
]

in_dir = 'ui/assets/img'
out_dir = 'ui/assets/img_bgra'

os.makedirs(out_dir, exist_ok=True)

for icon in icons:
    in_path = os.path.join(in_dir, icon)
    out_path = os.path.join(out_dir, icon.replace('.png', '.bgra'))
    
    img = PIL.Image.open(in_path).convert('RGBA')
    
    # We need to rotate the image 90 degrees CCW because mpv OSD is native portrait, 
    # and we want it to display correctly in landscape mode.
    # Wait, mpv OSD is 480x640 (portrait).
    # Landscape Top-Left is Physical Top-Left (Wait! No, Landscape Left maps to Physical Top).
    # Let's think:
    # nx goes Top to Bottom (0 to 480). ny goes Right to Left (0 to 640).
    # This means ny is horizontal, nx is vertical on the physical screen.
    # If the user holds it in landscape:
    # Vertical (nx) becomes Horizontal!
    # Horizontal (ny) becomes Vertical!
    # Wait, my mapped coordinates:
    # vx (Left-to-Right) maps to ny (Right-to-Left). So vx and ny are antiparallel.
    # vy (Top-to-Bottom) maps to nx (Top-to-Bottom). So vy and nx are parallel.
    # Wait, if vx maps to ny, and vy maps to nx.
    # Then X-axis becomes Y-axis! So the image needs to be rotated 90 degrees!
    # Which way?
    # Angle 0 in ASS points DOWN. To point RIGHT, we used \frz90 (90 CCW).
    # So we need to rotate the image 90 degrees CCW!
    # Resize icons to 64x64 to make them smaller
    img = img.resize((64, 64), PIL.Image.Resampling.LANCZOS)
    img = img.rotate(90, expand=True)
    
    # Convert RGBA to BGRA with Pre-multiplied Alpha
    # mpv's overlay-add expects the color channels to be pre-multiplied by the alpha.
    import numpy as np
    arr = np.array(img, dtype=np.float32)
    
    r = arr[:, :, 0]
    g = arr[:, :, 1]
    b = arr[:, :, 2]
    a = arr[:, :, 3]
    
    # Premultiply
    r = (r * a / 255.0).astype(np.uint8)
    g = (g * a / 255.0).astype(np.uint8)
    b = (b * a / 255.0).astype(np.uint8)
    a = a.astype(np.uint8)
    
    # Pack into BGRA
    bgra = np.stack([b, g, r, a], axis=-1)
    
    with open(out_path, 'wb') as f:
        f.write(bgra.tobytes())
    
    print(f"Saved {out_path} (64x64)")
