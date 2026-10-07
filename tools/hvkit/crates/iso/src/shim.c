/* The libisofs calls of the iso crate. struct burn_source is only reachable from C, so the whole
 * image is built here; lib.rs passes the options in. */
#include <stdint.h>   /* libisofs.h needs it first */
#define LIBISOFS_WITHOUT_LIBBURN
#include <libisofs/libisofs.h>
#include <stdio.h>
#include <stdlib.h>

struct hvkit_iso_opts {
    const char *volume_id;
    const char *bios_boot;     /* path in the tree of a no-emulation boot image, or NULL */
    int boot_load_size;        /* in 512-byte sectors */
    const char *efi_boot;      /* path in the tree of an EFI System Partition image, or NULL */
    const char *catalog;       /* path of the boot catalog in the tree */
    const char *const *hide;   /* NULL-terminated paths hidden from the ISO 9660 tree (not Joliet) */
    int iso_level;
    int rock_ridge, joliet;
    int force_dots;            /* "WIN51." instead of "WIN51" */
};

#define CHECK(x) do { int r_ = (x); if (r_ < 0) { snprintf(err, errlen, "%s: %s (%d)", #x, iso_error_to_msg(r_), r_); goto out; } } while (0)

int hvkit_iso_build(const char *root, const char *out_path, const struct hvkit_iso_opts *o, char *err, size_t errlen)
{
    IsoImage *img = NULL;
    IsoWriteOpts *w = NULL;
    struct burn_source *bs = NULL;
    FILE *f = NULL;
    int ret = -1;
    unsigned char buf[2048];

    CHECK(iso_init());
    CHECK(iso_image_new(o->volume_id, &img));
    CHECK(iso_tree_add_dir_rec(img, iso_image_get_root(img), root));
    if (o->bios_boot) {
        ElToritoBootImage *boot;
        CHECK(iso_image_set_boot_image(img, o->bios_boot, ELTORITO_NO_EMUL, o->catalog, &boot));
        el_torito_set_load_size(boot, o->boot_load_size);
    }
    if (o->efi_boot) {
        ElToritoBootImage *efi;
        if (o->bios_boot) {
            CHECK(iso_image_add_boot_image(img, o->efi_boot, ELTORITO_NO_EMUL, 0, &efi));
        } else {
            CHECK(iso_image_set_boot_image(img, o->efi_boot, ELTORITO_NO_EMUL, o->catalog, &efi));
        }
        CHECK(el_torito_set_boot_platform_id(efi, 0xef));
        /* The entry's sector count covers the whole image, as xorriso writes it: firmware takes the
         * FAT image to be that long (0 would leave it empty). */
        el_torito_set_full_load(efi, 1);
    }
    for (const char *const *h = o->hide; h && *h; h++) {
        IsoNode *node = NULL;
        CHECK(iso_tree_path_to_node(img, *h, &node));
        if (node)
            iso_node_set_hidden(node, LIBISO_HIDE_ON_RR);
    }
    if (o->bios_boot || o->efi_boot)
        CHECK(iso_image_set_boot_catalog_hidden(img, LIBISO_HIDE_ON_RR));

    CHECK(iso_write_opts_new(&w, 0));
    CHECK(iso_write_opts_set_iso_level(w, o->iso_level));
    CHECK(iso_write_opts_set_rockridge(w, o->rock_ridge));
    CHECK(iso_write_opts_set_joliet(w, o->joliet));
    CHECK(iso_write_opts_set_joliet_long_names(w, 1));
    CHECK(iso_write_opts_set_allow_deep_paths(w, 1));
    CHECK(iso_write_opts_set_omit_version_numbers(w, 1));
    CHECK(iso_write_opts_set_no_force_dots(w, o->force_dots ? 0 : 1));
    CHECK(iso_write_opts_set_allow_7bit_ascii(w, 1));

    CHECK(iso_image_create_burn_source(img, w, &bs));
    /* "wb" truncates an existing file instead of replacing it, which keeps its ACL (Hyper-V adds one
     * for the VM that has the image in its DVD drive). */
    if (!(f = fopen(out_path, "wb"))) {
        snprintf(err, errlen, "%s: cannot open for writing", out_path);
        goto out;
    }
    for (;;) {
        int n = bs->read_xt(bs, buf, sizeof(buf));
        if (n < 0) {
            snprintf(err, errlen, "libisofs failed while writing the image");
            goto out;
        }
        if (n == 0)
            break;
        if (fwrite(buf, 1, (size_t)n, f) != (size_t)n) {
            snprintf(err, errlen, "%s: write failed", out_path);
            goto out;
        }
    }
    if (fclose(f) != 0) {
        f = NULL;
        snprintf(err, errlen, "%s: write failed", out_path);
        goto out;
    }
    f = NULL;
    ret = 0;
out:
    if (f)
        fclose(f);
    if (bs) {
        bs->free_data(bs);
        free(bs);
    }
    if (w)
        iso_write_opts_free(w);
    if (img)
        iso_image_unref(img);
    iso_finish();
    return ret;
}
