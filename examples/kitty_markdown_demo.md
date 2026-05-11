# Kitty Markdown Demo

Plain Markdown text should continue through the existing VT100 stream.

| Item | Expected LCD result |
| --- | --- |
| Text | normal characters |
| Table | markdown table remains text |
| Code | fenced block remains text |
| Image | rendered by SG2002 Kitty PNG path |

```python
def hello_keyboard():
    print("VT100 text stays readable")
```

![verified SG2002 kitty render](./kitty_demo_image.ppm)

Text after the image should appear below the image placement.
