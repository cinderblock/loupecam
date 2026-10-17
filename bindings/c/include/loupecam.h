/*
 * LoupeCam C API: a driver for AmScope / ToupTek USB3 microscope cameras.
 * https://github.com/cinderblock/loupecam
 *
 * Generated from bindings/c/src/lib.rs by cbindgen; don't edit by hand.
 *
 * Every function returns a LoupecamStatus (LOUPECAM_STATUS_OK = 0) unless noted; on failure,
 * loupecam_last_error() describes it. Handles are opaque and freed with their
 * loupecam_*_free function. Functions may be called from any thread.
 *
 * The ABI may change before 1.0: rebuild against the header that ships with the
 * library.
 */

#ifndef LOUPECAM_H
#define LOUPECAM_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// Result of every call.
typedef enum LoupecamStatus {
  LOUPECAM_STATUS_OK = 0,
  // Anything not covered below (USB, camera, encoding, file I/O, internal).
  LOUPECAM_STATUS_ERROR = 1,
  // A NULL pointer where one isn't allowed, or a value out of range.
  LOUPECAM_STATUS_INVALID_ARGUMENT = 2,
  // No frame within the timeout.
  LOUPECAM_STATUS_TIMEOUT = 3,
  // The stream ended: the camera was stopped, restarted or freed.
  LOUPECAM_STATUS_STREAM_STOPPED = 4,
  // No camera (at that index).
  LOUPECAM_STATUS_NOT_FOUND = 5,
  // The call needs a running stream.
  LOUPECAM_STATUS_NOT_STREAMING = 6,
  // Not available on this camera (e.g. the simulator).
  LOUPECAM_STATUS_UNSUPPORTED = 7,
} LoupecamStatus;

// Pixel delivery modes (`LoupecamStartOptions.pixel_mode`).
typedef enum LoupecamPixelMode {
  // Raw sensor data, 8 bits per sample.
  LOUPECAM_PIXEL_MODE_RAW8 = 0,
  // Raw sensor data, 12 significant bits in 16-bit samples.
  LOUPECAM_PIXEL_MODE_RAW12 = 1,
  // The camera's own ISP (white balance, colour matrix, tone), 8 bits, still a mosaic.
  LOUPECAM_PIXEL_MODE_PROCESSED8 = 2,
} LoupecamPixelMode;

// Demosaic algorithms (`LoupecamDevelopOptions.demosaic`).
typedef enum LoupecamDemosaic {
  // Bilinear interpolation (the default).
  LOUPECAM_DEMOSAIC_BILINEAR = 0,
  // Each 2×2 cell becomes one pixel: half size, fastest.
  LOUPECAM_DEMOSAIC_SUPERPIXEL = 1,
  // Malvar-He-Cutler: sharpest, about twice the cost of bilinear.
  LOUPECAM_DEMOSAIC_MHC = 2,
} LoupecamDemosaic;

// White balance (`LoupecamDevelopOptions.white_balance`).
typedef enum LoupecamWhiteBalance {
  // The model's daylight gains (the default).
  LOUPECAM_WHITE_BALANCE_DAYLIGHT = 0,
  // Grey world over the whole frame.
  LOUPECAM_WHITE_BALANCE_AUTO = 1,
  // Unity gains.
  LOUPECAM_WHITE_BALANCE_NONE = 2,
  // `LoupecamDevelopOptions.white_balance_gains`.
  LOUPECAM_WHITE_BALANCE_GAINS = 3,
} LoupecamWhiteBalance;

// Colour matrix (`LoupecamDevelopOptions.color_matrix`).
typedef enum LoupecamColorMatrix {
  // The model's own (the default).
  LOUPECAM_COLOR_MATRIX_MODEL = 0,
  // None.
  LOUPECAM_COLOR_MATRIX_IDENTITY = 1,
  // `LoupecamDevelopOptions.color_matrix_values`, row-major.
  LOUPECAM_COLOR_MATRIX_CUSTOM = 2,
} LoupecamColorMatrix;

// Tone curves (`LoupecamDevelopOptions.tone`).
typedef enum LoupecamTone {
  // sRGB (the default).
  LOUPECAM_TONE_SRGB = 0,
  LOUPECAM_TONE_LINEAR = 1,
  // A power law with `LoupecamDevelopOptions.gamma`.
  LOUPECAM_TONE_GAMMA = 2,
} LoupecamTone;

// Image file formats.
typedef enum LoupecamFormat {
  LOUPECAM_FORMAT_PNG = 0,
  LOUPECAM_FORMAT_TIFF = 1,
  LOUPECAM_FORMAT_JPEG = 2,
} LoupecamFormat;

// An encoded file in memory.
typedef struct LoupecamBytes LoupecamBytes;

// An open camera.
typedef struct LoupecamCamera LoupecamCamera;

// One frame: the raw Bayer mosaic as the sensor delivered it.
typedef struct LoupecamFrame LoupecamFrame;

// A developed image.
typedef struct LoupecamImage LoupecamImage;

// Frames from a running camera.
typedef struct LoupecamStream LoupecamStream;

// A rectangle in pixels. A zero width or height means "none" where a region is
// optional.
typedef struct LoupecamRegion {
  uint32_t x;
  uint32_t y;
  uint32_t width;
  uint32_t height;
} LoupecamRegion;

// Options for `loupecam_camera_start`; start from `loupecam_start_options_default()`.
typedef struct LoupecamStartOptions {
  // Index into the model's sizes (0, the default, is the largest).
  uint32_t size;
  // A `LoupecamPixelMode` (default RAW8).
  uint32_t pixel_mode;
  // Exposure in µs (default 20000).
  uint32_t exposure_us;
  // Gain multiplier (default 1).
  double gain;
  // Readout speed, 0 (slowest) to 3 (fastest, the default).
  uint32_t speed;
  bool binning;
  // Stream only this region of the size (zero width: the whole frame).
  struct LoupecamRegion roi;
} LoupecamStartOptions;

// Develop options; start from `loupecam_develop_options_default()`, which gives the
// camera model's own settings: its colour matrix and daylight white balance, bilinear
// demosaicing, the sRGB curve, upright, 8 bits.
typedef struct LoupecamDevelopOptions {
  // A `LoupecamDemosaic`.
  uint32_t demosaic;
  // A `LoupecamWhiteBalance`.
  uint32_t white_balance;
  // R, G, B gains for `LOUPECAM_WHITE_BALANCE_GAINS`.
  float white_balance_gains[3];
  // A `LoupecamColorMatrix`.
  uint32_t color_matrix;
  // Row-major 3×3 matrix for `LOUPECAM_COLOR_MATRIX_CUSTOM`, applied to linear
  // camera RGB.
  float color_matrix_values[9];
  // A `LoupecamTone`.
  uint32_t tone;
  // The exponent for `LOUPECAM_TONE_GAMMA`.
  float gamma;
  // Input level (0-1) mapped to black.
  float black_point;
  // Input level (0-1) mapped to white.
  float white_point;
  // -1 to 1.
  float brightness;
  // -1 to 1.
  float contrast;
  // Sensor black level to subtract, in the frame's sample units.
  uint32_t black_level;
  // A single luminance channel.
  bool monochrome;
  bool negative;
  bool flip_horizontal;
  // Relative to upright (a bottom-up sensor readout is corrected regardless).
  bool flip_vertical;
  // Clockwise degrees: 0, 90, 180 or 270.
  uint32_t rotation;
  // Output bits per channel: 8 or 16.
  uint32_t depth;
} LoupecamDevelopOptions;

// Identity of an open camera and its model.
typedef struct LoupecamCameraInfo {
  char model[32];
  char serial[64];
  char firmware_version[32];
  char hardware_version[32];
  uint8_t fpga_version_major;
  uint8_t fpga_version_minor;
  uint16_t sensor_id;
  uint16_t vendor_id;
  uint16_t product_id;
  // Number of sizes; see `loupecam_camera_resolution`.
  uint32_t resolution_count;
  // Pixel pitch in micrometres.
  float pixel_size_um;
  // Significant bits per sample in the RAW12 mode.
  uint32_t max_bit_depth;
  // The mosaic's top-left 2×2 cell, e.g. "RGGB"; empty for monochrome sensors.
  char bayer_pattern[8];
  bool simulated;
} LoupecamCameraInfo;

// Geometry and timing of a running stream.
typedef struct LoupecamGeometry {
  uint32_t width;
  uint32_t height;
  // Significant bits per sample (8 or 12).
  uint32_t bits;
  // The exposure actually programmed, in µs.
  double exposure_us;
  // The highest frame rate this geometry allows at that exposure.
  double max_fps;
} LoupecamGeometry;

// Counters for a running stream.
typedef struct LoupecamStreamStats {
  uint64_t frames;
  // Frames dropped because the reader didn't keep up (the queue holds 3).
  uint64_t dropped;
  // Transfers or partial frames discarded while (re)synchronising.
  uint64_t discarded;
  uint64_t bytes;
  uint64_t transfer_errors;
  // Frames missing according to the camera's sequence counter.
  uint64_t sequence_gaps;
  // Frame rate over the last second.
  double fps;
} LoupecamStreamStats;

// A frame's layout.
typedef struct LoupecamFrameInfo {
  uint32_t width;
  uint32_t height;
  // Significant bits per sample: 8, or 12 in RAW12.
  uint32_t bits;
  // 1 (uint8_t samples) or 2 (little-endian uint16_t samples).
  uint32_t bytes_per_sample;
  // The camera's frame counter.
  uint32_t sequence;
} LoupecamFrameInfo;

// Statistics of a frame's raw mosaic.
typedef struct LoupecamFrameStats {
  // Mean linear level of R, G and B, 0 to 1.
  float mean[3];
  // Mean luma (Rec. 709) after white balance, 0 to 1.
  float luma;
  // Fraction of 2×2 cells with a channel at or near full scale.
  float clipped;
  // 256-bin histogram of per-cell linear luma.
  uint32_t histogram[256];
} LoupecamFrameStats;

// A developed image's layout. Pixels are interleaved, rows top to bottom, no padding.
typedef struct LoupecamImageInfo {
  uint32_t width;
  uint32_t height;
  // 3 (RGB), or 1 with `monochrome`.
  uint32_t channels;
  // Bits per channel: 8 (uint8_t) or 16 (uint16_t, native byte order).
  uint32_t depth;
} LoupecamImageInfo;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// The library version, e.g. "0.3.0". Static; never NULL.
const char *loupecam_version(void);

// A description of the last failure on this thread. Valid until the next failing
// call on this thread; never NULL (empty if nothing has failed).
const char *loupecam_last_error(void);

// The largest gain multiplier the camera accepts.
double loupecam_max_gain(void);

// The defaults for `loupecam_camera_start`.
struct LoupecamStartOptions loupecam_start_options_default(void);

// The defaults for developing: the camera model's own settings.
struct LoupecamDevelopOptions loupecam_develop_options_default(void);

// The number of connected cameras of supported models.
enum LoupecamStatus loupecam_camera_count(size_t *count);

// Open the connected camera at `index` (0: the first one found). Free it with
// `loupecam_camera_free`.
enum LoupecamStatus loupecam_camera_open(size_t index, struct LoupecamCamera **camera);

// Open a simulated MU1803-HS: synthetic frames, no hardware needed.
enum LoupecamStatus loupecam_camera_open_simulated(struct LoupecamCamera **camera);

// Stop streaming and release the camera. Streams from it end; frames already read
// stay valid.
void loupecam_camera_free(struct LoupecamCamera *camera);

enum LoupecamStatus loupecam_camera_info(const struct LoupecamCamera *camera, struct LoupecamCameraInfo *info);

// The model's size number `index` (0 is the largest).
enum LoupecamStatus loupecam_camera_resolution(const struct LoupecamCamera *camera, uint32_t index, uint32_t *width, uint32_t *height);

// Start streaming (`options` NULL: the defaults). Starting again reconfigures the
// camera, and earlier streams end. Free the stream with `loupecam_stream_free`.
enum LoupecamStatus loupecam_camera_start(struct LoupecamCamera *camera, const struct LoupecamStartOptions *options, struct LoupecamStream **stream);

// Stop streaming (the camera stays open).
enum LoupecamStatus loupecam_camera_stop(struct LoupecamCamera *camera);

// Set the exposure in µs; `actual_us` (may be NULL) receives the exposure actually
// programmed.
enum LoupecamStatus loupecam_camera_set_exposure(struct LoupecamCamera *camera, uint32_t exposure_us, double *actual_us);

// Set the gain as a multiplier (1 is unity, up to `loupecam_max_gain()`).
enum LoupecamStatus loupecam_camera_set_gain(struct LoupecamCamera *camera, double gain);

// Readout speed, 0 (slowest) to 3 (fastest).
enum LoupecamStatus loupecam_camera_set_speed(struct LoupecamCamera *camera, uint32_t speed);

enum LoupecamStatus loupecam_camera_set_binning(struct LoupecamCamera *camera, bool binning);

// Stream only a region of the current size (NULL or zero width: the whole frame).
enum LoupecamStatus loupecam_camera_set_roi(struct LoupecamCamera *camera, const struct LoupecamRegion *roi);

// The running stream's geometry (`LOUPECAM_STATUS_NOT_STREAMING` when stopped).
enum LoupecamStatus loupecam_camera_geometry(const struct LoupecamCamera *camera, struct LoupecamGeometry *geometry);

// The next frame, waiting up to `timeout_ms` (negative: forever). Free it with
// `loupecam_frame_free`.
enum LoupecamStatus loupecam_stream_read(const struct LoupecamStream *stream, int32_t timeout_ms, struct LoupecamFrame **frame);

// The newest frame, discarding older queued ones (for live views), waiting up to
// `timeout_ms` (negative: 1 s).
enum LoupecamStatus loupecam_stream_latest(const struct LoupecamStream *stream, int32_t timeout_ms, struct LoupecamFrame **frame);

enum LoupecamStatus loupecam_stream_stats(const struct LoupecamStream *stream, struct LoupecamStreamStats *stats);

void loupecam_stream_free(struct LoupecamStream *stream);

enum LoupecamStatus loupecam_frame_info(const struct LoupecamFrame *frame, struct LoupecamFrameInfo *info);

// The raw mosaic: `width × height` samples, row-major, as the sensor sends them
// (bottom-up on the MU1803; developing turns them upright). Valid until the frame is
// freed.
enum LoupecamStatus loupecam_frame_data(const struct LoupecamFrame *frame, const void **data, size_t *len);

// Develop into an image (`options` NULL: the defaults). Free it with
// `loupecam_image_free`.
enum LoupecamStatus loupecam_frame_develop(const struct LoupecamFrame *frame, const struct LoupecamDevelopOptions *options, struct LoupecamImage **image);

// Develop and encode (`format` a `LoupecamFormat`; `quality` 1-100 for JPEG). With
// `raw`, the undeveloped mosaic scaled to 16 bits, as TIFF only. Free the result with
// `loupecam_bytes_free`.
enum LoupecamStatus loupecam_frame_encode(const struct LoupecamFrame *frame, uint32_t format, uint32_t quality, bool raw, const struct LoupecamDevelopOptions *options, struct LoupecamBytes **bytes);

// Develop and write a file at `path` (UTF-8); the format comes from the extension
// (.png, .tif/.tiff, .jpg/.jpeg). `quality` and `raw` as for `loupecam_frame_encode`.
enum LoupecamStatus loupecam_frame_save(const struct LoupecamFrame *frame, const char *path, uint32_t quality, bool raw, const struct LoupecamDevelopOptions *options);

// Grey-world white balance: `gains` (3 floats) receives R, G, B. Over a neutral
// `region` (NULL: the whole frame), this is "click to white balance".
enum LoupecamStatus loupecam_frame_white_balance(const struct LoupecamFrame *frame, const struct LoupecamRegion *region, float *gains);

// Raw statistics over `region` (NULL: the whole frame). `white_balance` (3 floats,
// NULL: the model's daylight gains) weights the luma estimate.
enum LoupecamStatus loupecam_frame_stats(const struct LoupecamFrame *frame, const struct LoupecamRegion *region, const float *white_balance, struct LoupecamFrameStats *stats);

void loupecam_frame_free(struct LoupecamFrame *frame);

enum LoupecamStatus loupecam_image_info(const struct LoupecamImage *image, struct LoupecamImageInfo *info);

// The pixels (`len` in bytes). Valid until the image is freed.
enum LoupecamStatus loupecam_image_data(const struct LoupecamImage *image, const void **data, size_t *len);

void loupecam_image_free(struct LoupecamImage *image);

// The encoded file. Valid until the bytes are freed.
enum LoupecamStatus loupecam_bytes_data(const struct LoupecamBytes *bytes, const uint8_t **data, size_t *len);

void loupecam_bytes_free(struct LoupecamBytes *bytes);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* LOUPECAM_H */
