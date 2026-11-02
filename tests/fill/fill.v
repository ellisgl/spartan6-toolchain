// Capacity test: N flip-flops, each fed by its own small LUT, so it only fits
// the xc6slx9 (5720 LUT6 sites, 5720 main FFs + 5720 5FFs) when LUTs pair up
// into LUT6/LUT5 sites and the paired FFs go on 5FFs.
//
// r[i] toggles when r[i-1] and en are both high (a ripple of toggle stages);
// each stage's LUT reads r[i], r[i-1] and en, so neighbours share inputs.
// The LEDs show an XOR of slices of r so no stage is optimised away.
module fill #(parameter N = 5800) (
    input  wire       clk,
    input  wire       rst_n,
    output wire [7:0] led
);
    reg [N-1:0] r = 0;
    reg [23:0] div = 0;
    always @(posedge clk) div <= div + 1'b1;
    wire en = div[23] & rst_n;

    integer i;
    always @(posedge clk) begin
        r[0] <= r[0] ^ en;
        for (i = 1; i < N; i = i + 1)
            r[i] <= r[i] ^ (r[i-1] & en);
    end

    genvar k;
    generate
        for (k = 0; k < 8; k = k + 1) begin : g
            assign led[k] = ^r[k*N/8 +: N/8];
        end
    endgenerate
endmodule
