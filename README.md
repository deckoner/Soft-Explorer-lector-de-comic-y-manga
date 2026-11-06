# Soft Explorer

**Soft Explorer es un lector de escritorio para quienes ya tienen su colección de manga y quieren leerla sin depender de una web, una cuenta o la nube.** Seleccionas una carpeta local y la aplicación organiza las obras por género, conserva tus favoritos y recuerda por dónde ibas.

Está construido con **Rust y Tauri 2**, con una interfaz en **React y TypeScript**. Su aspecto Retro-Modern Geek combina crema, rojo óxido, coral, azul y amarillo, con DM Sans, Bricolage Grotesque y sombras sólidas desplazadas.

## Qué puedes hacer

- Explorar una biblioteca con estructura `Género/Obra/páginas`, buscar por nombre y ordenar por título, género, favoritos o lectura reciente.
- Guardar favoritos, elegir una portada de la propia obra y continuar desde la última página.
- Leer en fullscreen, pasar páginas con teclado, ratón o rueda, ajustar el tamaño, elegir dirección occidental u oriental y usar doble página.
- Activar la presentación automática y ajustar el intervalo; empieza en **5 segundos**.
- Reescanear de forma incremental. La cuadrícula está virtualizada y las miniaturas se generan bajo demanda.

Admite JPG/JPEG, PNG, WEBP, BMP, GIF y TIFF. GIF se muestra como fotograma estático; la lectura de AVIF depende del soporte disponible en el sistema.

## Los originales se quedan donde están

La aplicación solo lee la biblioteca: no mueve, renombra ni edita sus archivos. La base de datos, las preferencias, las miniaturas y las páginas adaptadas se guardan en el directorio de datos de Soft Explorer del sistema operativo.

Desarrollo y release usan espacios de datos separados (`development` y `release`). Así, las pruebas y su configuración de biblioteca no aparecen en la instalación final. Los datos antiguos no se borran.

## Empezar a usarlo

Al abrir la aplicación, pulsa **Elegir biblioteca** y selecciona la carpeta que contiene los géneros. Por ejemplo:

```text
Biblioteca/
├── Acción/
│   ├── Obra A/
│   │   ├── 001.jpg
│   │   └── 002.webp
│   └── Obra B/
│       └── página.png
└── Romance/
    └── Obra C/
        └── 01.jpg
```

El escaneo se ejecuta en segundo plano y muestra su progreso. Los datos quedan en ese equipo y la aplicación no necesita conexión a Internet para leer.

## Desarrollo

### Requisitos

- Node.js 20.19+ o 22.12+.
- Rust estable instalado mediante rustup.
- **Windows:** Visual Studio 2022 con _Desktop development with C++_, MSVC x64/x86, Windows 10/11 SDK y WebView2 Runtime.
- **Linux:** Rust, Node y las dependencias Tauri/WebKitGTK 4.1 de la distribución.

En Windows, desde PowerShell en la raíz del proyecto:

```powershell
npm ci
.\run-dev.ps1
```

`run-dev.ps1` configura Cargo y el entorno de Visual Studio para esa sesión. En Linux:

```bash
npm ci
npm run tauri:dev
```

En Debian o Ubuntu recientes, las dependencias nativas se instalan con:

```bash
sudo apt update
sudo apt install -y build-essential curl wget file libssl-dev libxdo-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf libwebkit2gtk-4.1-dev
```

Otras distribuciones necesitan los paquetes equivalentes de WebKitGTK 4.1 y GTK.

## Formatear y comprobar el código

Prettier se ocupa de React, TypeScript, CSS y archivos de configuración; rustfmt, del backend Rust. Los comandos se ejecutan desde la raíz:

```bash
npm run format          # formatea frontend, configuración y Rust
npm run format:check    # comprueba ambos formatos
npm run format:web      # solo frontend y configuración web
npm run format:rust     # solo Rust (cargo fmt)
```

El formateador Rust busca Cargo en `%USERPROFILE%\.cargo\bin` en Windows aunque no esté en el `PATH`. Si falta rustfmt: `rustup component add rustfmt`.

## Crear instaladores

### Windows

```powershell
.\build-windows.ps1
```

El script prepara el entorno de Visual Studio, compila en release y genera MSI y NSIS. También puedes ejecutar `npm run tauri:build` desde una terminal de Visual Studio Developer.

### Linux

Ejecuta la compilación desde Linux; Tauri no compila instaladores Linux desde Windows:

```bash
npm run tauri:build
```

Los paquetes disponibles para esa distribución aparecen en `src-tauri/target/release/bundle/`. En Windows, los instaladores se guardan en las subcarpetas `msi/` y `nsis/` del mismo directorio.

La compilación de Windows se ha probado en este equipo. La generación del paquete Linux está configurada, pero debe validarse en una máquina Linux.

## Licencia

El código de Soft Explorer se distribuye bajo **GNU GPL versión 3 o posterior** (`GPL-3.0-or-later`). Si distribuyes una versión modificada del programa, debes conservar esa licencia y ofrecer el código fuente correspondiente. La GPL protege la libertad de usar, estudiar, modificar y compartir el software; no prohíbe cobrar por copias o servicios. Las imágenes y mangas de cada biblioteca no forman parte de este proyecto ni quedan cubiertos por su licencia.
