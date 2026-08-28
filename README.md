# image-to-vector

Расширения к движку [VTracer](https://github.com/visioncortex/vtracer) 1.0,
закрывающие то, чего в нём нет: корректная работа с альфа-каналом
(прозрачные PNG без ореола), геометрическая регуляризация контуров и,
возможно, градиенты.

VTracer 1.0 — это framework с публичными точками расширения (`Frontend`,
`ColorFitter`, `CurvePass`, `CurveFitter`, `OptimizerPass`), поэтому проект
строится как набор плагинов к нему, а не как обёртка и не как форк.

Rust, консоль — не веб. Техническое задание и статус по каждому пункту —
[`docs/SPEC.md`](docs/SPEC.md).

## Использование

```sh
cargo run -p i2v-cli -- in.png out.svg                          # ванильный VTracer 1.0
cargo run -p i2v-cli -- in.png out.svg --defringe                # + альфа-осведомлённый фронтенд (v1, нативное разрешение)
cargo run -p i2v-cli -- in.png out.svg --supersample 4            # v2: субпиксельный контур (медленнее, точнее — не для pixel art)
cargo run -p i2v-cli -- in.png out.svg --regularize               # + окружности/оси, композируется с любым из выше
cargo run -p i2v-cli -- in.png out.svg --defringe --regularize --save-profile mine.json   # сохранить настройки
cargo run -p i2v-cli -- in.png out.svg --profile mine.json                                 # воспроизвести дословно
cargo run -p i2v-cli -- corpus/ out/ --profile mine.json                                    # батч: директория → директория + report.csv
cargo run -p i2v-bench --bin gen_corpus                           # (пере)сгенерировать синтетический корпус
cargo run -p i2v-bench                                            # quality gate: mean_err/p99/SSIM vs vanilla, exit≠0 при регрессии
cargo test --workspace
```

## Статус

- **Модуль A (альфа-канал)** — v1 и v2 реализованы и протестированы:
  `crates/i2v-core/src/lib.rs` (v1, нативное разрешение), `supersample.rs`
  (v2, субпиксельный контур через supersampling — измеримо лучше v1 на всех
  альфа-кейсах, `pixel-art` осознанно исключён).
- **Модуль C (регуляризация геометрии)** — реализован и измерен:
  `regularize.rs`. Окружности + оси, 0 регрессий на всём корпусе; симметрия и
  согласование радиусов архитектурно недоступны как `CurvePass`, не
  реализованы (см. `docs/SPEC.md` §3).
- **Профиль настроек + батч** — реализован: `profile.rs` (JSON, round-trip
  проверен тестами), `--profile`/`--save-profile` в CLI, батч по директории
  с CSV-отчётом. См. `docs/SPEC.md` §7.
- **Бенчмарк с метрикой качества** — реализован: рендер SVG обратно в растр
  (`resvg`), ошибка RGBA + SSIM против оригинала, правило приёмки как код,
  подключено в CI. 0 регрессий на 14 файлах корпуса. См. `docs/SPEC.md` §6.
- **Модуль B (градиенты)** — не начат, требует форка VTracer (единственный
  модуль, который не плагин), см. `docs/SPEC.md` §4/§9 — сессия
  остановлена перед ним, нужно отдельное решение о запуске.

## Лицензия

MIT (совместима с VTracer, MIT OR Apache-2.0).
