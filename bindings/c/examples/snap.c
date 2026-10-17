/*
 * Take a picture: open a camera (or the simulator), stream, develop, save.
 *
 *   snap [--simulated] [output.png]
 *
 * Also the C API's test (bindings/c/test.sh runs it against the simulator), so it
 * checks results more thoroughly than an example needs to.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "loupecam.h"

/* Abort with the library's description of what went wrong. */
static void check(LoupecamStatus s, const char *what) {
  if (s != LOUPECAM_STATUS_OK) {
    fprintf(stderr, "%s failed (status %d): %s\n", what, (int)s, loupecam_last_error());
    exit(1);
  }
}

static void expect(int ok, const char *what) {
  if (!ok) {
    fprintf(stderr, "unexpected: %s\n", what);
    exit(1);
  }
}

int main(int argc, char **argv) {
  int simulated = argc > 1 && strcmp(argv[1], "--simulated") == 0;
  const char *out = argc > 1 + simulated ? argv[1 + simulated] : "snap.png";

  printf("LoupeCam %s\n", loupecam_version());

  LoupecamCamera *cam = NULL;
  if (simulated) {
    check(loupecam_camera_open_simulated(&cam), "open the simulator");
  } else {
    check(loupecam_camera_open(0, &cam), "open the first camera");
  }

  LoupecamCameraInfo info;
  check(loupecam_camera_info(cam, &info), "camera info");
  printf("%s serial %s, %s mosaic, %u sizes\n", info.model, info.serial, info.bayer_pattern, info.resolution_count);

  /* The smallest size, 12-bit. */
  uint32_t size = info.resolution_count - 1, width, height;
  check(loupecam_camera_resolution(cam, size, &width, &height), "resolution");

  LoupecamStartOptions start = loupecam_start_options_default();
  start.size = size;
  start.pixel_mode = LOUPECAM_PIXEL_MODE_RAW12;
  start.exposure_us = 10000;
  LoupecamStream *stream = NULL;
  check(loupecam_camera_start(cam, &start, &stream), "start");

  LoupecamGeometry geom;
  check(loupecam_camera_geometry(cam, &geom), "geometry");
  printf("streaming %ux%u, %u-bit, up to %.1f fps\n", geom.width, geom.height, geom.bits, geom.max_fps);
  expect(geom.width == width && geom.height == height, "geometry matches the size");

  LoupecamFrame *frame = NULL;
  check(loupecam_stream_read(stream, 5000, &frame), "read a frame");

  LoupecamFrameInfo fi;
  check(loupecam_frame_info(frame, &fi), "frame info");
  const void *raw;
  size_t raw_len;
  check(loupecam_frame_data(frame, &raw, &raw_len), "frame data");
  expect(fi.bits == 12 && fi.bytes_per_sample == 2, "12-bit samples");
  expect(raw_len == (size_t)fi.width * fi.height * 2, "raw size");
  printf("frame #%u: %ux%u\n", fi.sequence, fi.width, fi.height);

  /* Develop: the model's defaults, with grey-world white balance and MHC. */
  float gains[3];
  check(loupecam_frame_white_balance(frame, NULL, gains), "white balance");
  printf("grey-world gains %.2f %.2f %.2f\n", gains[0], gains[1], gains[2]);

  LoupecamDevelopOptions dev = loupecam_develop_options_default();
  dev.demosaic = LOUPECAM_DEMOSAIC_MHC;
  dev.white_balance = LOUPECAM_WHITE_BALANCE_GAINS;
  memcpy(dev.white_balance_gains, gains, sizeof gains);

  LoupecamImage *img = NULL;
  check(loupecam_frame_develop(frame, &dev, &img), "develop");
  LoupecamImageInfo ii;
  const void *pixels;
  size_t pixels_len;
  check(loupecam_image_info(img, &ii), "image info");
  check(loupecam_image_data(img, &pixels, &pixels_len), "image data");
  expect(ii.channels == 3 && ii.depth == 8 && pixels_len == (size_t)ii.width * ii.height * 3, "RGB image");
  loupecam_image_free(img);

  check(loupecam_frame_save(frame, out, 90, false, &dev), "save");
  printf("saved %s\n", out);

  LoupecamBytes *jpeg = NULL;
  const uint8_t *bytes;
  size_t bytes_len;
  check(loupecam_frame_encode(frame, LOUPECAM_FORMAT_JPEG, 85, false, NULL, &jpeg), "encode");
  check(loupecam_bytes_data(jpeg, &bytes, &bytes_len), "bytes");
  expect(bytes_len > 2 && bytes[0] == 0xff && bytes[1] == 0xd8, "a JPEG");
  loupecam_bytes_free(jpeg);

  /* Errors are reported, not crashes. */
  dev.rotation = 45;
  expect(loupecam_frame_develop(frame, &dev, &img) == LOUPECAM_STATUS_INVALID_ARGUMENT, "bad rotation rejected");
  expect(strstr(loupecam_last_error(), "rotation") != NULL, "the error names the option");
  expect(loupecam_frame_develop(NULL, NULL, &img) == LOUPECAM_STATUS_INVALID_ARGUMENT, "NULL frame rejected");
  start.pixel_mode = 99;
  LoupecamStream *bad = NULL;
  expect(loupecam_camera_start(cam, &start, &bad) == LOUPECAM_STATUS_INVALID_ARGUMENT, "bad pixel mode rejected");

  loupecam_frame_free(frame);

  /* Stopping ends the stream. */
  check(loupecam_camera_stop(cam), "stop");
  LoupecamStatus s;
  do {
    s = loupecam_stream_read(stream, 1000, &frame);
    if (s == LOUPECAM_STATUS_OK) loupecam_frame_free(frame);
  } while (s == LOUPECAM_STATUS_OK);
  expect(s == LOUPECAM_STATUS_STREAM_STOPPED, "the stream stopped");

  loupecam_stream_free(stream);
  loupecam_camera_free(cam);
  loupecam_camera_free(NULL); /* harmless */
  printf("ok\n");
  return 0;
}
