// VREF I/O standard SSTL2_I (bank 3): inputs (differential to the bank's
// VREF) and outputs. Checked against ISE bitgen only (the Mojo V3's banks
// run at 3.3 V); ISE places the bank's VREF pins.
module iostdsstl (input wire [1:0] i, output wire [3:0] o);
    assign o = {i[0], i[1], i[0] ^ i[1], ~i[0]};
endmodule
