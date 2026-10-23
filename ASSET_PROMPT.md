# Ассеты лаунчера LimeCraft: промпт для генерации изображений

> Имя приложения: **LimeCraft** (утверждено пользователем 19.10.2026).
> Вставьте блок ниже в любой генератор изображений (Midjourney, DALL·E,
> Flux, Kandinsky). Промпт описывает стиль, палитру, размеры. В конце —
> вариации под каждую конкретную ассету.
>
> Палитра по хексу (чтобы генератор не «плыл»): `#7FBF5F`, `#5E9E4C`,
> `#F0E39B`, `#E2C96A`, `#F6FBF0`, `#EDF6E3`, текст `#2E3B2C`.

---

## ОСНОВНОЙ ПРОМПТ (стилевой «якорь» для всей серии ассетов)

```
Minimalist flat design app icon for a Minecraft launcher named "LimeCraft",
color palette: light fresh green #7FBF5F, deeper green #5E9E4C, soft butter
yellow #F0E39B and golden yellow #E2C96A, on a very pale misty green-white
background #F6FBF0. Concept: a single rounded square tile containing one
clean simple symbol — no text, no clutter, plenty of whitespace, subtle soft
shadow, thin rounded corners bucket. Style: modern minimal, gradients barely
perceptible, matte, vector-like, crisp edges, high detail on the symbol only.
Aesthetic: airy, calm, positive, spring-like. 1024x1024 PNG, centered,
transparent background. --no watermark
```

---

## Вариации для конкретных ассетов (подставляйте блок над этим)

### 1. Иконка приложения (главная, для таскбара)

Замените «single clean simple symbol» на:
- символ «куб-трава с ростком»: `a stylized green grass block with one small
  sprout of two leaves sprouting out of the top, yellow sun dot behind`,
- чисто однолистный росток: `a single minimalist young sprout with two leaves,
  one leaf green one leaf yellow, emerging from a tiny soil mound`.

### 2. Логотип-бейдж (в левом верхнем углу окна)

```
Square app emblem, rounded corners, flat: a small green grass block icon with a
yellow sprout, minimalist vector style, single accent green outline on white,
no text, suitable as in-app badge, 256x256.
```

### 3. Фон-герой (главный экран / экран загрузки)

```
Wide minimalist hero background, 1920x1080, landscape, very pale misty
green-white gradient, subtle tiny pixel-lawn texture at the bottom edge, a few
soft blurred circles in light green and butter yellow floating, lots of empty
space in the center for UI, flat matte, calming, no text.
```

### 4. Кнопка «Играть» (и её иконка)

Сначала генерируем иконку кнопки, затем для кнопки просто:
yellow minimal rounded play: `a polygon play triangle inside a soft rounded
square, gradient from #7FBF5F to #E2C96A, flat, minimalist, no shadow clump`.

### 5. Плитки модов (ассеты под мод-иконки)

```
Set of 16 tiny flat minimal tile icons for a mod manager, 96x96 each, one
per tile, colors only from the palette #7FBF5F #5E9E4C #F0E39B #E2C96A #FFF:
a gear wrench, a pickaxe, an anvil, a chest, a book, a sword, a potion, a shovel,
a bow, a hoe, a compass, a map, a clock, a trap, a beacon, a lantern,
rounded square background per tile, flat, no text, icon grid 4x4 arrangement,
sprite sheet with 12px margins between tiles and 24px outer padding.
```

### 6. Иконка / мотив для «сборок» (Modrinth)

```
Minimal flat glyph: a stack of three rounded blocks (green, yellow, pale green),
isometric pseudo, thin outline, on transparent background, 256x256.
```

### 7. Пустое состояние (нет модов / нет сборок)

```
Flat minimal illustration, empty state: a single sad-ish flowered sapling in a
small soil mound, pale green background, yellow blossom, 2D flat vector style,
no text, no people, calm, 800x600.
```

### 8. Фавиконка для веб/окна (ico)

```
Simple friendly 64x64 leaf mark, green, on transparent, flat, minimal.
```

---

## Чек-лист качества во всех генерациях

- [ ] Только цвета из палитры (не более 3 оттенков + фон + текст).
- [ ] Плоский flat-стиль, без 3D-перегруза, без градиентов «радуга».
- [ ] Без текста, без водяных знаков, без лишних элементов.
- [ ] Прозрачный фон для иконок (PNG); для фонов — ровный градиент.
- [ ] Достаточно воздуха вокруг символа (≈20–25% отступы).

## Куда складывать

```
assets/
├─ icon-512.png        # приложение (таскбар)
├─ icon-256.png        # окно
├─ badge-256.png       # лого-бейдж в шапке
├─ hero-1920x1080.png  # фон главного экрана и загрузки
├─ play-256.png        # значок кнопки «Играть»
├─ tile-*.png          # инстанс-ассеты (placeholder при отсутствии иконки мода)
├─ empty-*.png         # пустые состояния
├─ favicon-64.ico      # окно
└─ sprite-modtiles.png # (опционально) спрайт плиток модов
```

Если нет доступа к генератору прямо сейчас: можно нарисовать SVG-копии
всех базовых иконок (куб-росток, play, шестерёнка, кирка) — они лягут в
`assets/` и будут использованы в UI, а сгенерированные позже заменят их.
Опционально: базовые SVG-дубликаты будут добавлены при реализации UI.
