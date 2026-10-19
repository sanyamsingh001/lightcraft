# LightCraft en español

Elige **Editar → Idioma → Español** o **Ajustes → General → Idioma**.
El cambio es inmediato y se guarda para el próximo inicio (`"language": "es"` en `ui.json`).
El comando correspondiente es `app.language.spanish`.

Los catálogos españoles traducen las 1.750 claves de la base del catálogo de referencia
alemán (que usa las claves originales en inglés), más las cinco etiquetas de tipos de
archivo compartidas con la revisión de importación: 1.755 claves en total. Traducen los
180 mensajes con formato de esa base y los tres resúmenes de carpetas compartidos con
la misma revisión (183 en total). Incluyen menús,
controles de edición, recorte, máscaras, biblioteca, importación y exportación, ajustes,
personas, reglas de álbumes inteligentes, ajustes preestablecidos, perfiles, historial,
versiones y las líneas de Novedades presentes en el catálogo. Las fechas se muestran, por ejemplo, como
`domingo, 20/09/2026`; los valores ISO guardados no cambian.

Se usa un español común a distintas regiones. Se aceptan `es`, `es-ES`, `es-MX`,
`es_AR.UTF-8` y otras variantes regionales; siempre se guarda el código `es`.
Los nombres de archivo, identificadores de comandos, marcadores de plantilla y nombres
creados por el usuario permanecen intactos. Los ejemplos de descripciones para SAM 3
conservan sus términos en inglés y el texto escrito por el usuario no se traduce.
Las fuentes Inter existentes contienen los caracteres españoles: no se añaden fuentes
ni recursos gráficos.

Las cadenas nuevas que aún no estén en el catálogo y los errores técnicos de las capas
inferiores siguen usando el texto original en inglés. La cobertura del catálogo actual
no implica que se hayan traducido todos los posibles errores ni verificado todos los diseños.

## Mantenimiento y comprobación

Textos fijos: `crates/ui-egui/locales/es.json`. Mensajes con valores:
`crates/ui-egui/locales/es-formats.json`. Las claves en inglés no se modifican.
Se conservan los argumentos, signos y precisiones de formato. Las terminaciones plurales
compatibles se usan directamente; las que no corresponden se consumen con `{:.0}` y
se emplean mensajes de recuento, como `Ajustes preestablecidos importados: 3`.

```sh
cargo test -p lightcraft-ui-egui i18n::tests
LIGHTCRAFT_LANGUAGE=es lightcraft-cli snapshot --demo -o espanol.png --size 1600x1000
```

Los tests añadidos comprueban normalización, preferencias, cambio de idioma por menú y
canal de control, listado estable de comandos, restauración del estado en una sesión sin
ventana, fechas, plurales, valores numéricos y texto dibujado con los caracteres españoles.
También comprueban que los nombres y metadatos del usuario no se traduzcan.
Más detalles en [localization.md](localization.md).
