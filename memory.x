MEMORY
{
  /* On-chip Flash memory */
  FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 512k
  /* On-chip SRAM */
  RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 256k
}
