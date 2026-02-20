# Reference

## CLI

**TODO:** generate this automatically but in structured HTML

### test

Usage: `bombadil test [OPTIONS] <ORIGIN> [SPECIFICATION_FILE]`

Arguments:

* `<ORIGIN>`
 Starting URL of the test (also used as a boundary so that Bombadil doesn't navigate to other websites)

* `[SPECIFICATION_FILE]`
  A custom specification in TypeScript or JavaScript, using the `@antithesishq/bombadil` package on NPM

Options:

* `--output-path <OUTPUT_PATH>`
      Where to store output data (trace, screenshots, etc)
* `--exit-on-violation`
      Whether to exit the test when first failing property is found (useful in development and CI)
* `--width <WIDTH>`
      Browser viewport width in pixels [default: 1024]
* `--height <HEIGHT>`
      Browser viewport height in pixels [default: 768]
* `--device-scale-factor <DEVICE_SCALE_FACTOR>`
      Scaling factor of the browser viewport, mostly useful on high-DPI monitors when in headed mode [default: 2]
* `--headless`
      Whether the browser should run in a visible window or not
* `--no-sandbox`
      Disable Chromium sandboxing
* `-h, --help`
          Print help
