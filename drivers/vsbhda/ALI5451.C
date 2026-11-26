/* External 32-bit VSBHDA backend for a Ferric-prepared ALi M5451. */
#include <stddef.h>
#include <stdint.h>
#include <conio.h>
#include "AUISTRUC.H"
#include <stdio.h>
#include "LINEAR.H"
#include "DMABUFF.H"
#include "AC97MIX.H"
#include "DPMIDRV.H"

struct ali_card {
    struct cardmem_s dma;
    unsigned port, revision, size;
};

/* Preparing the DynaBook makes its legacy DSP reset port answer. Call the
 * JLM here, after VSBHDA has checked for an existing Sound Blaster. */
extern unsigned int _linear_rmstack;
extern unsigned char bOMode;

static int prepare_controller(struct ali_card *card)
{
    __dpmi_regs lookup = {0}, call = {0}, status = {0};
    unsigned char *info;
    if (!_linear_rmstack || _linear_rmstack > 0xffc00UL) {
        printf("ALI5451: no DOS transfer buffer\n");
        return 0;
    }
    /* JLOAD intercepts an INT instruction executed by a V86 client.
     * Calling the real-mode IVT handler via DPMI 0300h skips that hook.
     * Run INT 2Fh; RETF in the loader's DOS transfer buffer via 0301h. */
    info = NearPtr(_linear_rmstack);
    info[0] = 0xcd;
    info[1] = 0x2f;
    info[2] = 0xcb;
    lookup.x.cs = _linear_rmstack >> 4;
    lookup.x.ip = _linear_rmstack & 15;
    lookup.x.ax = 0x1684;
    lookup.x.bx = 0xfd01;
    if (__dpmi_simulate_real_mode_procedure_retf(&lookup) ||
        !(lookup.x.es | lookup.x.di)) {
        printf("ALI5451: FERRIC.JLM lookup failed\n");
        return 0;
    }
    call.x.cs = lookup.x.es;
    call.x.ip = lookup.x.di;
    call.x.ax = 0x002b;
    if (__dpmi_simulate_real_mode_procedure_retf(&call) || call.x.ax) {
        printf("ALI5451: prepare failed AX=%X\n", (unsigned)call.x.ax);
        return 0;
    }
    /* AX=22h writes 40 bytes of AudioInfo to ES:DI. VSBHDA's 1 KiB DOS
     * transfer area is available until installation completes. SS:SP=0
     * asks HDPMI to use its own stack for these simulated far calls. */
    status.x.cs = lookup.x.es;
    status.x.ip = lookup.x.di;
    status.x.es = _linear_rmstack >> 4;
    status.x.di = _linear_rmstack & 15;
    status.x.ax = 0x0022;
    if (__dpmi_simulate_real_mode_procedure_retf(&status) || status.x.ax) {
        printf("ALI5451: status failed AX=%X\n", (unsigned)status.x.ax);
        return 0;
    }
    info = NearPtr(_linear_rmstack);
    card->revision = info[4];
    card->port = ReadLinearW(_linear_rmstack + 8);
    printf("ALI5451: port=%X IRQ=%u rev=%u\n", card->port, (unsigned)info[5], card->revision);
    return info[0] && info[5] == 11 && card->port && card->port <= 0xfeff;
}

static int detect(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    unsigned period = aui->card_period_size ? aui->card_period_size : 512;

    /* DOS stdout is available during detection, before going resident. */
    bOMode = 1;
    printf("ALI5451: detecting via FERRIC.JLM\n");
    if (period > 65536 || period % 4 || !prepare_controller(card))
        return 0;

    /* Voice 0 interrupts at the midpoint and end of the ring. */
    card->size = 2 * period;
    if (!MDma_alloc_cardmem(&card->dma, card->size)) {
        printf("ALI5451: DMA allocation failed\n");
        return 0;
    }
    aui->card_irq = 11;
    aui->card_buffers = 2;
    aui->card_pDmaBuffer = card->dma.pMem;
    return 1;
}

static void stop(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    unsigned p = card->port;
    outpd(p + 0xa4, 0);
    outpd(p + 0x84, 1);
    outpd(p + 0x98, 1);
    outpd(p + 0xb0, 0x8c00);
}

static void close_card(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    if (card && card->port) {
        stop(aui);
        MDma_free_cardmem(&card->dma);
        card->port = 0;
    }
}

static void setrate(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    if (aui->freq_card < 4000) aui->freq_card = 4000;
    if (aui->freq_card > 48000) aui->freq_card = 48000;
    MDma_initbuf(aui, card->size);
}

static void start(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    unsigned p = card->port;
    unsigned delta = (aui->freq_card * 4096UL + 24000) / 48000;
    stop(aui);
    outp(p + 0xa0, 0);
    outpd(p + 0xe0, 0);
    outpd(p + 0xe4, card->dma.physicalptr);
    outpd(p + 0xe8, ((card->size / 4 - 1) << 16) | delta);
    outpd(p + 0xf0, 0x8000f000); /* signed 16-bit stereo, looping, centered */
    outpd(p + 0xf4, 0x30000000);
    outpd(p + 0xf8, 0x30000000);
    outpd(p + 0xa0, inpd(p + 0xa0) | 0x3000);
    outpd(p + 0xa4, 1);
    outpd(p + 0x80, 1);
}

static unsigned position(struct audioout_info_s *aui)
{
    struct ali_card *card = aui->card_private_data;
    outp(card->port + 0xa0, 0);
    return (inpw(card->port + 0xe2) * 4U) % card->size;
}

static int interrupt_card(struct ali_card *card)
{
    unsigned p = card->port;
    unsigned misc = inpd(p + 0xb0);
    unsigned voice = (misc & 0x20) && (inpd(p + 0x98) & 1);
    if (voice) outpd(p + 0x98, 1);
    if (misc & 0x8c00) outpd(p + 0xb0, misc & 0x8c00);
    return voice;
}

/* Serialize each AC'97 transaction against BUSY and the sample timer. */
static int codec_ready(unsigned p, unsigned reg)
{
    unsigned i, tick = inpd(p + 0xc8);
    for (i = 0; i < 65536; ++i)
        if (!(inpd(p + reg) & 0x8000) && inpd(p + 0xc8) != tick)
            return 1;
    return 0;
}

static void mixer_write(struct audioout_info_s *aui, unsigned long reg, unsigned long val)
{
    struct ali_card *card = aui->card_private_data;
    if (reg < 0x80 && codec_ready(card->port, 0x40))
        outpd(card->port + 0x40, (val << 16) | 0x8000 | reg |
              (card->revision == 2 ? 0x100 : 0));
}

static unsigned long mixer_read(struct audioout_info_s *aui, unsigned long reg)
{
    struct ali_card *card = aui->card_private_data;
    unsigned offset = card->revision == 2 ? 0x40 : 0x44;
    if (reg >= 0x80 || !codec_ready(card->port, offset)) return 0xffff;
    outpd(card->port + offset, 0x8000 | reg);
    if (!codec_ready(card->port, offset)) return 0xffff;
    return inpd(card->port + offset) >> 16;
}

const struct sndcard_info_s ALI5451_sndcard_info = {
    "ALI5451", 0, detect, start, stop, close_card, setrate,
    NULL, position, interrupt_card, mixer_write, mixer_read,
    aucards_ac97chan_mixerset, sizeof(struct ali_card)
};
